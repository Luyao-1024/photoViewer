# Photo Viewer

Photo Viewer 是一个基于 GNOME GTK4 + Libadwaita 的本地相册应用，面向大规模照片库的浏览、查看、相册、回收站、编辑和视频/动态照片播放。

## Status

**0.9.0 release candidate**: browsing, thumbnails, viewer, albums, trash, editor, video playback, motion photo playback, theme preferences, and Flathub-oriented packaging metadata are implemented.

## Features

- Photos: Year / Month / Day views over the local library
- Albums: folder-derived albums with cover thumbnails and reorder support
- Trash: system trash integration with multi-select restore/delete flows
- Viewer: image, video, motion photo, details panel, keyboard shortcuts
- Editor: rotate, crop, brightness, contrast, saturation, save paths
- Appearance: light/dark/system theme and optional Liquid Glass material

## Quick Start

Install system dependencies from [docs/development.md](docs/development.md), then use the normal Cargo loop:

```bash
cargo build
cargo run
cargo test
```

`cargo build` runs `build.rs`, compiles `data/ui/*.blp` templates, and bundles UI/icon resources. Flatpak install/update, X11 visual checks, and trash-portal reproduction commands also live in [docs/development.md](docs/development.md).

## Documentation

Start from [docs/README.md](docs/README.md). Current behavior is documented by functional module under `docs/modules/`; historical design specs and implementation plans are archived under `docs/superpowers/` for background context only.

Agent workflow rules live in [AGENTS.md](AGENTS.md). Contributor workflow lives in [CONTRIBUTING.md](CONTRIBUTING.md).

## Language Configuration

The app ships built-in `zh-CN` and `en` text. You can switch language with an environment variable:

```bash
PHOTO_VIEWER_LOCALE=en cargo run
```

For persistent overrides, copy one of the examples from `config/` to:

```text
~/.config/io.github.luyao_1024.photoviewer/i18n.json
```

## Changelog

See [CHANGELOG.md](CHANGELOG.md).
