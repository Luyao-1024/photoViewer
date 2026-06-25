#!/usr/bin/env bash
# Incrementally build in the GNOME 50 SDK, then run in the PhotoViewer sandbox.
#
# This is the development runner. It avoids flatpak-builder for the edit/run
# loop because flatpak-builder recreates the module build root often enough to
# throw away Cargo's registry and target caches. Building directly in the SDK
# keeps incremental artifacts while using the GNOME 50 toolchain:
#
#   - target/flatpak-debug        (compiled Rust artifacts)
#   - ~/.cache/photoViewer/cargo-home (Cargo registry/git downloads)
#
# The binary is then launched through the installed org.gnome.PhotoViewer app
# sandbox, not the SDK sandbox. GNOME 50's gdk-pixbuf/glycin image loaders fail
# when nested under the generic SDK app-id, which makes thumbnails stay white.
#
# Use `CLEAN=1 ./run-flatpak.sh` to remove target/flatpak-debug before running.
set -euo pipefail

cd "$(dirname "$0")"

PROJECT_DIR="$(pwd)"
CACHE_ROOT="${XDG_CACHE_HOME:-$HOME/.cache}/photoViewer"
CARGO_HOME_DIR="$CACHE_ROOT/cargo-home"
TARGET_DIR="target/flatpak-debug"

mkdir -p "$CARGO_HOME_DIR"

if [ -n "${CLEAN:-}" ]; then
    echo "==> clean rebuild requested; removing $TARGET_DIR..."
    rm -rf "$TARGET_DIR"
fi

echo "==> cargo build in GNOME 50 SDK sandbox..."
flatpak run \
    --devel \
    --share=network \
    --filesystem="$PROJECT_DIR" \
    --filesystem="$CARGO_HOME_DIR" \
    --env=PROJECT_DIR="$PROJECT_DIR" \
    --env=CARGO_HOME="$CARGO_HOME_DIR" \
    --env=CARGO_TARGET_DIR="$TARGET_DIR" \
    --env=PATH="/usr/lib/sdk/rust-stable/bin:/app/bin:/usr/bin" \
    --command=sh org.gnome.Sdk//50 \
    -lc 'cd "$PROJECT_DIR" && cargo build --locked'

echo "==> run photo-viewer in app sandbox..."
exec flatpak run \
    --filesystem="$PROJECT_DIR" \
    --filesystem=home \
    --command="$PROJECT_DIR/$TARGET_DIR/debug/photo-viewer" \
    org.gnome.PhotoViewer
