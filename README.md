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
- Sync: native SyncServer with selected-album upload, complete-photo download, favorites, conflict recovery and explicit cloud deletion

## Quick Start

Install system dependencies from [docs/development.md](docs/development.md), then use the normal Cargo loop:

```bash
cargo build
cargo run
cargo test
```

`cargo build` runs `build.rs`, compiles `data/ui/*.blp` templates, and bundles UI/icon resources. Flatpak install/update, X11 visual checks, and trash-portal reproduction commands also live in [docs/development.md](docs/development.md).

## Cloud Sync

Cloud synchronization uses the companion native Rust SyncServer. Settings provides
username/password login and an HTTPS capability probe, configured zone selection and an explicit upload
album selection. Empty selection receives cloud photos only. Complete original
and thumbnail pairs are published to the local gallery, with automatic checks,
manual synchronization, pause/resume, conflict recovery and explicit cloud deletion.
Login sessions remain in the system keyring; passwords are not persisted.
Different accounts on the same server use separate tasks and local roots, with
identity checks, sign-out and same-account reauthentication. See the
[account integration guide](docs/modules/native-sync-accounts.md). Native Backup is not exposed.

See [the client usage guide](docs/modules/native-sync-usage.md) for setup and
[the implementation record](docs/modules/native-sync-integration.md) for the exact
focused and live validation evidence. The authoritative protocol is owned by the
sibling server and linked from [the contract index](docs/modules/native-sync-protocol.md).

## Logs and Local Cleanup

Settings now groups preferences in a category sidebar. **Storage and Logs** lets
you change the logging level immediately. Its inline **Data Cleanup** group shows
usage and provides separate actions for thumbnails + media metadata, logs,
accounts, or all application data. Library/log cleanup preserves login; account
and all-data cleanup remove both local account records and system-keyring tokens.
Original photos and videos are preserved.

Each launch writes a timestamped plain-text log under
`$XDG_CACHE_HOME/io.github.luyao_1024.photoviewer/logs/`. On startup, logs dated
before today are compressed into daily gzip archives; today's earlier runs stay
uncompressed. Total log storage is capped at 500 MB and can be reduced in Data
Cleanup. See [diagnostics](docs/modules/diagnostics.md) for paths and retention.

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

原生同步已接入设置、后台调度和逐照片发布；数据库版本固定为 1，开发阶段不维护旧结构迁移兼容。按用户要求只运行相关聚焦测试，完整远端发布门禁另行执行。实现与实机证据见[修订说明](docs/modules/native-sync-integration.md)，开发分支保持 dev。
