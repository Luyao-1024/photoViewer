#!/usr/bin/env bash
# Reproduce and log Flatpak Trash portal behavior from the app sandbox.
#
# This intentionally does not call host `gio trash`: host GIO bypasses the
# Flatpak Trash portal and cannot reproduce Flathub/user-install failures.
set -euo pipefail

APP_ID="${FLATPAK_APP_ID:-io.github.luyao_1024.photoviewer}"
PICTURES_DIR="${XDG_PICTURES_DIR:-$HOME/Pictures}"
STAMP="$(date +%Y%m%d-%H%M%S)"
TEST_FILE="$PICTURES_DIR/photo-viewer-flatpak-trash-repro-$STAMP.txt"

log() {
    printf '[flatpak-trash-repro] %s\n' "$*"
}

cleanup_original() {
    if [ -e "$TEST_FILE" ]; then
        rm -f "$TEST_FILE"
        log "cleanup: removed original test file"
    fi
}

cleanup_trash_entry() {
    local trash_root="${XDG_DATA_HOME:-$HOME/.local/share}/Trash"
    local info
    info="$(find "$trash_root/info" -maxdepth 1 -type f \
        -name "$(basename "$TEST_FILE").trashinfo" -print -quit 2>/dev/null || true)"
    if [ -n "$info" ]; then
        rm -f "$trash_root/files/$(basename "$TEST_FILE")" "$info"
        log "cleanup: removed test entry from host trash"
    fi
}

require_command() {
    if ! command -v "$1" >/dev/null 2>&1; then
        log "missing required command: $1"
        exit 127
    fi
}

require_command flatpak
mkdir -p "$PICTURES_DIR"

log "app_id=$APP_ID"
log "pictures_dir=$PICTURES_DIR"
log "test_file=$TEST_FILE"

log "flatpak info:"
flatpak info "$APP_ID" | sed 's/^/[flatpak-trash-repro]   /'

log "flatpak permissions:"
flatpak info --show-permissions "$APP_ID" | sed 's/^/[flatpak-trash-repro]   /'

log "sandbox gio version:"
flatpak run --command=gio "$APP_ID" --version | sed 's/^/[flatpak-trash-repro]   /'

printf 'photo viewer flatpak trash portal repro\n' > "$TEST_FILE"
log "created test file"

log "sandbox gio info:"
flatpak run --command=gio "$APP_ID" info "$TEST_FILE" \
    | grep -E 'uri:|local path:|standard::name|access::can-trash' \
    | sed 's/^/[flatpak-trash-repro]   /' || true

log "sandbox gio trash:"
set +e
TRASH_OUTPUT="$(flatpak run --command=gio "$APP_ID" trash "$TEST_FILE" 2>&1)"
TRASH_STATUS=$?
set -e
printf '%s\n' "$TRASH_OUTPUT" | sed 's/^/[flatpak-trash-repro]   /'
log "trash_exit_code=$TRASH_STATUS"

if [ -e "$TEST_FILE" ]; then
    log "host_file_exists=yes"
else
    log "host_file_exists=no"
fi

if [ "$TRASH_STATUS" -eq 0 ]; then
    cleanup_trash_entry
else
    cleanup_original
fi

if [ "$TRASH_STATUS" -ne 0 ]; then
    log "result=FAIL: sandbox GIO trash failed through Flatpak Trash portal"
else
    log "result=OK: sandbox GIO trash succeeded through Flatpak Trash portal"
fi

exit "$TRASH_STATUS"
