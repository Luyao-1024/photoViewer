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

## 本地开发构建

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

说明：

- `cargo build` 会运行 `build.rs`，自动把 `data/ui/*.blp` 编译成 `.ui`，并把 UI 与图标打包进 GResource。
- 本地 `cargo run` 使用宿主系统 GTK/libadwaita，适合日常逻辑调试。
- Liquid Glass 的 `backdrop-filter` 视觉效果需要 GTK 4.22+；如果宿主 GTK 较旧，请使用下面的 Flatpak 安装运行。

## Flatpak 安装 / 更新

当前 Ubuntu 24.04 系统 GTK4 停留在 4.14.x。若要在不替换系统 GTK 的前提下使用更新的 GTK 运行时，使用项目根目录的 `org.gnome.PhotoViewer.yml` 构建 Flatpak。该清单固定到 `org.gnome.Platform//50`，并通过 Rust SDK 扩展在沙箱内构建 release 二进制。

```bash
sudo apt install flatpak-builder

flatpak remote-add --if-not-exists flathub \
  https://dl.flathub.org/repo/flathub.flatpakrepo

flatpak install flathub \
  org.gnome.Platform//50 \
  org.gnome.Sdk//50 \
  org.freedesktop.Sdk.Extension.rust-stable

flatpak-builder --user --install --ccache --disable-rofiles-fuse --force-clean \
  /tmp/photoViewer-flatpak-build org.gnome.PhotoViewer.yml

flatpak run org.gnome.PhotoViewer
```

说明：

- 构建阶段允许 `cargo` 联网拉取 crate，并使用 `cargo build --release --locked` 保持依赖版本与 `Cargo.lock` 一致。
- 运行阶段不替换系统 `/usr` 中的 GTK，应用使用 Flatpak runtime 内的 GTK/libadwaita。
- 沙箱授予 `home` 读写权限，因为应用需要浏览、编辑和管理本地照片。
- `--disable-rofiles-fuse` 用于规避本工作站上已知的 `rofiles-fuse` 卸载卡死问题；保留该参数不会影响正常安装。
- 使用 `/tmp/photoViewer-flatpak-build` 作为构建目录，可以避开仓库内旧 `.flatpak-builder` 状态。若确认本机没有遗留挂载，也可以换成普通的 `build-flatpak` 目录。

更新已安装应用时，在最新 `main` 上重新执行同一条 `flatpak-builder --user --install ...` 命令即可；安装结果可用下面命令确认：

```bash
flatpak info org.gnome.PhotoViewer
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

参见 [docs/README.md](docs/README.md)。当前文档按功能模块组织；历史设计 spec 和 milestone
plan 保留在 `docs/superpowers/` 中作为背景资料。

## Changelog

参见 [CHANGELOG.md](CHANGELOG.md)。

## GTK 4.22 liquid glass rendering

The mode selector glass surface is implemented with GTK 4.22+ native CSS `backdrop-filter` support inside the Flatpak runtime. The project targets `org.gnome.Platform//50`, so the app can use GTK 4.22 even when the host distribution ships an older GTK version.

The effect is intentionally implemented as an in-app backdrop blur:

- `box.mode-selector` uses `backdrop-filter: blur(...) saturate(...) brightness(...)` for the glass distortion layer.
- The translucent fill, border highlight, and inset shadows are tuned in `src/ui/grid_css.rs`.
- The previous CPU snapshot/refraction path has been removed to avoid manual background captures during scrolling.

This does not depend on host GTK 4.22. Build and run through Flatpak to use the private runtime:

```bash
flatpak-builder --user --install --ccache --disable-rofiles-fuse --force-clean \
  /tmp/photoViewer-flatpak-build org.gnome.PhotoViewer.yml
flatpak run org.gnome.PhotoViewer
```
