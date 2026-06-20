# Photo Viewer

基于 GNOME (GTK4 + Libadwaita) 的高性能相册工具。

## 状态

M1: 基础浏览（年/月/日视图 + 本地扫描 + SQLite 索引）

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