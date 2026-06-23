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

## 多语言配置

应用默认使用 `zh-CN`（中文）或 `en`（英文）文案。可通过配置文件切换语言和覆盖文案。

创建文件（示例值来自 `config/i18n.example.json`）：

`~/.config/photoViewer/i18n.json`

```json
{
  "locale": "en",
  "overrides": {
    "app.title": "Photo Viewer",
    "viewer.button.favorite": "Favorite",
    "viewer.button.favorite_active": "Unfavorite"
  }
}
```

字段说明：

- `locale`: `zh-CN` 或 `en`，决定内置语言包。
- `overrides`: key-value 形式覆盖内置文案（只影响你提供的 key）。

快速切换到英文（任选其一）：

1. 使用环境变量（仅本次启动生效）：
   ```bash
   PHOTO_VIEWER_LOCALE=en cargo run
   ```

2. 或写入配置文件（持久生效）：
   ```bash
   mkdir -p ~/.config/photoViewer
   cp config/i18n.en.example.json ~/.config/photoViewer/i18n.json
   ```

   然后重启应用即可看到英文界面。`locale: "en"` 会覆盖系统语言。

## 架构

参见 [spec](docs/superpowers/specs/2026-06-20-gnome-photo-viewer-design.md)
和 [M1 plan](docs/superpowers/plans/2026-06-20-m1-foundation-and-browsing.md)。

## Changelog

参见 [CHANGELOG.md](CHANGELOG.md)。
