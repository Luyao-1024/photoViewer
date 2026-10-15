# Photo Viewer

Photo Viewer 是一个基于 GNOME GTK4 + Libadwaita 的本地相册应用，面向大规模照片库的浏览、查看、相册、回收站、编辑和视频/动态照片播放。

## Status

**0.9.9 release candidate**: browsing, thumbnails, viewer, albums, trash, editor, video playback, motion photo playback, theme preferences, and Flathub-oriented packaging metadata are implemented.

## Features

- Photos: Year / Month / Day views over the local library
- Albums: folder-derived albums with cover thumbnails and reorder support
- Trash: system trash integration with multi-select restore/delete flows
- Viewer: image, video, motion photo, details panel, keyboard shortcuts
- Editor: rotate, crop, brightness, contrast, saturation, save paths
- Appearance: light/dark/system theme and optional Liquid Glass material
- Sync: native SyncServer client (connection and capability probe shipped; media synchronization in development)

## Quick Start

Install system dependencies from [docs/development.md](docs/development.md), then use the normal Cargo loop:

```bash
cargo build
cargo run
cargo test
```

`cargo build` runs `build.rs`, compiles `data/ui/*.blp` templates, and bundles UI/icon resources. Flatpak install/update, X11 visual checks, and trash-portal reproduction commands also live in [docs/development.md](docs/development.md).

## Cloud Sync

Cloud synchronization is being rebuilt around the native Rust
[`Luyao-1024/syncserver`](https://github.com/Luyao-1024/syncserver) service. The
previous WebDAV file-sync engine has been removed. The current build ships the
connection layer only: a server can be probed and its capability scope pinned,
but photos are not synchronized yet. The settings surface, receive cycle and
upload path arrive with the later C1 milestones — see
[docs/modules/native-sync.md](docs/modules/native-sync.md) for the contract and
[docs/modules/native-sync-plan.md](docs/modules/native-sync-plan.md) for the
delivery order.

## Documentation

Start from [docs/README.md](docs/README.md). Current behavior is documented by functional module under `docs/modules/`. Earlier milestone specs and plans are not kept in the tree; recover them from git history when you need the original rationale.

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


Native SyncServer 的协议唯一来源是服务端仓库，参见[服务端契约入口](docs/modules/native-sync-protocol.md)中的本地相对链接；正式发布时再统一改为网页链接。客户端维护自己的[接入方案](docs/modules/native-sync.md)、[本地数据与发布方案](docs/modules/native-sync-storage.md)、[实现修订](docs/modules/native-sync-integration.md)和[阶段验收](docs/modules/native-sync-plan.md)。

C1.1–C1.4 现有实现已按静态检视修订，schema 10 保留 schema 9 的原生数据。当前不将编译通过视为完整同步验收；C1.5 设置/交互和 C1.6 完整交付仍待完成。本轮按最新要求仅运行基本相关的聚焦用例，不运行全量测试；本地交付与验证记录见[修订说明](docs/modules/native-sync-integration.md)。客户端开发分支保持 dev。
