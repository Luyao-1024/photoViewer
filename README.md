# Photo Viewer

基于 GNOME (GTK4 + Libadwaita) 的高性能相册工具。

## Status

**M1-M5 complete** (0.5.0): Full browsing + thumbnails + viewer + albums + trash + editor + polish.

## Features

- 📷 Photos: Year / Month / Day views of all photos
- 📁 Albums: folder-as-album with cover thumbnails
- 🗑 Trash: System trash integration with multi-select restore/delete
- ✏ Edit: Rotate (destructive + 5s undo), Crop, Brightness, Contrast, Saturation
- ⚙️ Extensible EditOperation trait for future filters/effects
- 🌗 Dark/light theme follows system
- 🚀 1-10万张照片规模下流畅运行

## 构建

```bash
# 系统依赖 (Fedora)
sudo dnf install gtk4-devel libadwaita-devel gdk-pixbuf2-devel \
                 libheif-devel sqlite-devel

# 系统依赖 (Ubuntu)
sudo apt install libgtk-4-dev libadwaita-1-dev libgdk-pixbuf-2.0-dev \
                 libheif-dev libsqlite3-dev

cargo build
cargo run
```

## 测试

```bash
cargo test
```

## 架构

参见 [spec](docs/superpowers/specs/2026-06-20-gnome-photo-viewer-design.md)
和 [M1 plan](docs/superpowers/plans/2026-06-20-m1-foundation-and-browsing.md)。

## Changelog

参见 [CHANGELOG.md](CHANGELOG.md)。
