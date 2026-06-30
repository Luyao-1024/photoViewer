#!/usr/bin/env bash
# Run a small visual smoke check on X11 and skip on Wayland.
#
# The app's Liquid Glass rendering must be checked in the GNOME 50 Flatpak
# runtime. Wayland screenshot tooling differs by compositor, so this script
# intentionally limits automated screenshots to X11/Xvfb.
set -euo pipefail

cd "$(dirname "$0")/.."

OUTPUT_DIR="target/visual-checks"
TIMEOUT_SECONDS=20
KEEP_APP=0
RELEASE_BUILD=0
SELF_TEST=0

usage() {
    cat <<'USAGE'
Usage: tools/visual-check-x11.sh [options]

Options:
  --output-dir <dir>   Directory for screenshots (default: target/visual-checks)
  --timeout <seconds>  Window wait timeout (default: 20)
  --release            Build and run the release profile
  --keep-app           Leave the app running after the screenshot
  --self-test          Run script environment classifier tests
  -h, --help           Show this help

Behavior:
  - Wayland sessions are skipped with exit code 0.
  - X11 sessions use the current DISPLAY when available.
  - Headless non-Wayland runs start Xvfb and then run the same X11 check.
USAGE
}

while (( "$#" )); do
    case "$1" in
        --output-dir)
            if (( $# < 2 )); then
                echo "Missing argument for --output-dir" >&2
                exit 2
            fi
            OUTPUT_DIR="$2"
            shift 2
            ;;
        --timeout)
            if (( $# < 2 )); then
                echo "Missing argument for --timeout" >&2
                exit 2
            fi
            TIMEOUT_SECONDS="$2"
            shift 2
            ;;
        --release)
            RELEASE_BUILD=1
            shift
            ;;
        --keep-app)
            KEEP_APP=1
            shift
            ;;
        --self-test)
            SELF_TEST=1
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            echo "Unknown argument: $1" >&2
            usage
            exit 2
            ;;
    esac
done

classify_display() {
    local session_type="${1:-}"
    local display="${2:-}"

    case "${session_type,,}" in
        wayland)
            echo "wayland"
            ;;
        x11)
            echo "x11"
            ;;
        *)
            if [[ -n "$display" ]]; then
                echo "x11"
            else
                echo "headless"
            fi
            ;;
    esac
}

assert_classification() {
    local session_type="$1"
    local display="$2"
    local expected="$3"
    local actual
    actual="$(classify_display "$session_type" "$display")"
    if [[ "$actual" != "$expected" ]]; then
        echo "classification failed: session='$session_type' display='$display' expected='$expected' actual='$actual'" >&2
        return 1
    fi
}

run_self_test() {
    assert_classification "wayland" ":0" "wayland"
    assert_classification "x11" ":0" "x11"
    assert_classification "" ":99" "x11"
    assert_classification "" "" "headless"
    echo "visual-check-x11 self-test passed"
}

if (( SELF_TEST )); then
    run_self_test
    exit 0
fi

require_command() {
    local command_name="$1"
    local package_hint="$2"
    if ! command -v "$command_name" >/dev/null 2>&1; then
        echo "Missing dependency: $command_name ($package_hint)" >&2
        exit 1
    fi
}

SESSION_KIND="$(classify_display "${XDG_SESSION_TYPE:-}" "${DISPLAY:-}")"

if [[ "$SESSION_KIND" == "wayland" ]]; then
    echo "Skipping visual check: Wayland session detected. X11 visual checks only."
    exit 0
fi

require_command flatpak "flatpak"
require_command xdotool "xdotool"
require_command import "ImageMagick"
require_command xdpyinfo "x11-utils / xorg-x11-utils"

XVFB_PID=""
APP_PID=""
STARTED_APP=0

cleanup() {
    local status=$?
    if (( KEEP_APP == 0 )) && [[ -n "$APP_PID" ]]; then
        kill "$APP_PID" >/dev/null 2>&1 || true
        wait "$APP_PID" >/dev/null 2>&1 || true
    fi
    if [[ -n "$XVFB_PID" ]]; then
        kill "$XVFB_PID" >/dev/null 2>&1 || true
        wait "$XVFB_PID" >/dev/null 2>&1 || true
    fi
    exit "$status"
}
trap cleanup EXIT

if [[ "$SESSION_KIND" == "headless" ]]; then
    require_command Xvfb "xorg-x11-server-Xvfb / xvfb"
    export DISPLAY=":97"
    Xvfb "$DISPLAY" -screen 0 1440x960x24 -nolisten tcp >/tmp/photo-viewer-xvfb.log 2>&1 &
    XVFB_PID="$!"
    sleep 1
fi

if ! xdpyinfo >/dev/null 2>&1; then
    echo "X11 display is not usable: DISPLAY=${DISPLAY:-<unset>}" >&2
    exit 1
fi

if (( RELEASE_BUILD )); then
    ./run-flatpak.sh --release --no-audio >/tmp/photo-viewer-visual-check.log 2>&1 &
else
    ./run-flatpak.sh --no-audio >/tmp/photo-viewer-visual-check.log 2>&1 &
fi
APP_PID="$!"
STARTED_APP=1

find_window_id() {
    xdotool search --onlyvisible --class "photo-viewer" 2>/dev/null | tail -n 1 && return 0
    xdotool search --onlyvisible --class "org.gnome.PhotoViewer" 2>/dev/null | tail -n 1 && return 0
    xdotool search --onlyvisible --class "Photo Viewer" 2>/dev/null | tail -n 1 && return 0
    xdotool search --onlyvisible --name "Photo Viewer" 2>/dev/null | tail -n 1 && return 0
    return 1
}

WINDOW_ID=""
deadline=$((SECONDS + TIMEOUT_SECONDS))
while (( SECONDS < deadline )); do
    if ! kill -0 "$APP_PID" >/dev/null 2>&1; then
        echo "App exited before a window appeared. See /tmp/photo-viewer-visual-check.log" >&2
        exit 1
    fi
    WINDOW_ID="$(find_window_id || true)"
    if [[ -n "$WINDOW_ID" ]]; then
        break
    fi
    sleep 0.5
done

if [[ -z "$WINDOW_ID" ]]; then
    echo "Timed out waiting for Photo Viewer window. See /tmp/photo-viewer-visual-check.log" >&2
    exit 1
fi

mkdir -p "$OUTPUT_DIR"
TIMESTAMP="$(date +%Y%m%d-%H%M%S)"
SCREENSHOT="$OUTPUT_DIR/photo-viewer-x11-$TIMESTAMP.png"

xdotool windowactivate "$WINDOW_ID" >/dev/null 2>&1 || true
sleep 1
import -window "$WINDOW_ID" "$SCREENSHOT"

echo "Visual check screenshot saved: $SCREENSHOT"
if (( STARTED_APP )) && (( KEEP_APP )); then
    echo "Photo Viewer left running with PID $APP_PID"
fi
