#!/usr/bin/env bash
# Run a small visual smoke check on X11 and skip on Wayland.
#
# The app's Liquid Glass rendering must be checked in the GNOME 50 Flatpak
# runtime. Wayland screenshot tooling differs by compositor, so this script
# intentionally limits automated screenshots to X11/Xvfb.
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
SCRIPT_PATH="$SCRIPT_DIR/$(basename -- "${BASH_SOURCE[0]}")"
cd "$SCRIPT_DIR/.."

OUTPUT_DIR="target/visual-checks"
TIMEOUT_SECONDS=45
KEEP_APP=0
RELEASE_BUILD=0
SELF_TEST=0
KEYBOARD_SMOKE=0
A11Y_SMOKE=0
ORIGINAL_ARGS=("$@")

usage() {
    cat <<'USAGE'
Usage: tools/visual-check-x11.sh [options]

Options:
  --output-dir <dir>   Directory for screenshots (default: target/visual-checks)
  --timeout <seconds>  Startup/window wait timeout (default: 45)
  --release            Build and run the release profile
  --keep-app           Leave the app running after the screenshot
  --keyboard-smoke     Send Ctrl+F through X11 and capture the Search page
  --a11y-smoke         Assert Search-page AT-SPI roles, names, and focus
  --self-test          Run script environment classifier tests
  -h, --help           Show this help

Behavior:
  - Wayland sessions are skipped with exit code 0.
  - X11 sessions use the current DISPLAY when available.
  - Headless non-Wayland runs start Xvfb and then run the same X11 check.
  - --keyboard-smoke captures startup, then verifies the real X11 keyboard
    path can open Search without modifying library data.
  - --a11y-smoke implies --keyboard-smoke and checks the accessibility tree
    exposed by the running Flatpak after Search opens.
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
        --keyboard-smoke)
            KEYBOARD_SMOKE=1
            shift
            ;;
        --a11y-smoke)
            A11Y_SMOKE=1
            KEYBOARD_SMOKE=1
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

# Xvfb provides no session D-Bus. Start one and confirm the AT-SPI bus plus
# registry before launching the Flatpak, so GTK can expose accessibility data.
if [[ "${PHOTO_VIEWER_AT_SPI_READY:-}" != "1" ]]; then
    export PHOTO_VIEWER_AT_SPI_READY=1
    exec "$SCRIPT_DIR/with-at-spi.sh" "$SCRIPT_PATH" "${ORIGINAL_ARGS[@]}"
fi

require_command() {
    local command_name="$1"
    local package_hint="$2"
    if ! command -v "$command_name" >/dev/null 2>&1; then
        echo "Missing dependency: $command_name ($package_hint)" >&2
        exit 1
    fi
}

require_python_module() {
    local module_name="$1"
    local package_hint="$2"
    if ! python3 -c "import $module_name" >/dev/null 2>&1; then
        echo "Missing Python module: $module_name ($package_hint)" >&2
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
if (( A11Y_SMOKE )); then
    require_command python3 "python3-pyatspi"
    require_python_module pyatspi "python3-pyatspi"
fi

XVFB_PID=""
APP_PID=""
STARTED_APP=0

cleanup() {
    local status=$?
    if (( KEEP_APP == 0 )) && [[ -n "$APP_PID" ]]; then
        # run-flatpak.sh starts the app through several wrappers. It must have
        # its own process group so a completed smoke test cannot leave an
        # orphaned Flatpak window for the next run to target by mistake.
        kill -- "-$APP_PID" >/dev/null 2>&1 || true
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
    setsid ./run-flatpak.sh --release --no-audio >/tmp/photo-viewer-visual-check.log 2>&1 &
else
    setsid ./run-flatpak.sh --no-audio >/tmp/photo-viewer-visual-check.log 2>&1 &
fi
APP_PID="$!"
STARTED_APP=1

find_window_id() {
    xdotool search --onlyvisible --class "photo-viewer" 2>/dev/null | tail -n 1 && return 0
    xdotool search --onlyvisible --class "io.github.luyao_1024.photoviewer" 2>/dev/null | tail -n 1 && return 0
    xdotool search --onlyvisible --class "Photo Viewer" 2>/dev/null | tail -n 1 && return 0
    xdotool search --onlyvisible --name "Photo Viewer" 2>/dev/null | tail -n 1 && return 0
    return 1
}

wait_for_keyboard_router() {
    local deadline=$((SECONDS + TIMEOUT_SECONDS))
    while (( SECONDS < deadline )); do
        # MainWindow presents before its async storage initialization wires the
        # production keyboard router. The grid-renderer log is emitted at that
        # boundary, so waiting for it prevents a shortcut from being delivered
        # to an incomplete startup shell.
        if grep -q "photos grid renderer initialized" /tmp/photo-viewer-visual-check.log 2>/dev/null; then
            return 0
        fi
        sleep 0.5
    done
    echo "Timed out waiting for the Photo Viewer keyboard router to initialize. See /tmp/photo-viewer-visual-check.log" >&2
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

if (( KEYBOARD_SMOKE == 0 )); then
    xdotool windowactivate "$WINDOW_ID" >/dev/null 2>&1 || true
fi
sleep 1
import -window "$WINDOW_ID" "$SCREENSHOT"

echo "Visual check screenshot saved: $SCREENSHOT"
if (( KEYBOARD_SMOKE )); then
    # Target the app window directly. This also works under bare Xvfb, where
    # no window manager owns _NET_ACTIVE_WINDOW for `windowactivate`.
    wait_for_keyboard_router
    xdotool windowfocus "$WINDOW_ID" >/dev/null 2>&1 || true
    # XSendEvent delivery (`xdotool key --window`) is ignored by some GTK/X11
    # stacks. Use XTEST against the focused window so this is the same input
    # route as a physical keyboard.
    xdotool key ctrl+f
    sleep 1
    SEARCH_SCREENSHOT="$OUTPUT_DIR/photo-viewer-x11-search-$TIMESTAMP.png"
    import -window "$WINDOW_ID" "$SEARCH_SCREENSHOT"
    if [[ ! -s "$SEARCH_SCREENSHOT" ]]; then
        echo "Search keyboard smoke screenshot is empty: $SEARCH_SCREENSHOT" >&2
        exit 1
    fi
    if cmp -s "$SCREENSHOT" "$SEARCH_SCREENSHOT"; then
        if (( A11Y_SMOKE == 0 )); then
            echo "Ctrl+F did not visibly change the Photo Viewer window" >&2
            exit 1
        fi
        # Under bare Xvfb, two distinct Adw views can have identical raster
        # output. The following AT-SPI assertion is the authoritative signal
        # for --a11y-smoke, so do not turn that visual limitation into a false
        # failure.
        echo "Ctrl+F screenshot was unchanged; verifying the Search page through AT-SPI instead."
    fi
    echo "Keyboard smoke screenshot saved: $SEARCH_SCREENSHOT"
    if (( A11Y_SMOKE )); then
        a11y_args=(--timeout "$TIMEOUT_SECONDS")
        if [[ "${PHOTO_VIEWER_AT_SPI_DUMP:-}" == "1" ]]; then
            a11y_args+=(--dump)
        fi
        timeout "$((TIMEOUT_SECONDS + 5))s" python3 "$SCRIPT_DIR/assert-at-spi.py" "${a11y_args[@]}"
    fi
fi
if (( STARTED_APP )) && (( KEEP_APP )); then
    echo "Photo Viewer left running with PID $APP_PID"
fi
