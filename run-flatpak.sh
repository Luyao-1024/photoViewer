#!/usr/bin/env bash
# 一键编译并在 Flatpak (GNOME 50 runtime) 沙箱中运行 photo-viewer。
# 用法: ./run-flatpak.sh
set -euo pipefail

cd "$(dirname "$0")"

echo "==> cargo build..."
cargo build

echo "==> flatpak run..."
exec flatpak run \
  --filesystem="$(pwd)" \
  --filesystem=home \
  --command=sh org.gnome.PhotoViewer \
  -c "exec $(pwd)/target/debug/photo-viewer"
