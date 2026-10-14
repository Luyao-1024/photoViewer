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


Native SyncServer C1 has an implementation-ready
[client contract](docs/modules/native-sync.md),
[protocol mapping](docs/modules/native-sync-protocol.md),
[storage/publication design](docs/modules/native-sync-storage.md), and
[staged implementation/acceptance plan](docs/modules/native-sync-plan.md), aligned
with server M3/M4 at `syncserver@2d9f9a7`. PhotoViewer Native runtime is still
pending: this documentation does not implement lease/token/dirty coordination
or dual-resource publication.
Development work is pushed to `dev`, with CI using the same gate as `main`.
