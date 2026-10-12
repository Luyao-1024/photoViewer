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
- WebDAV: selected-album backup and bidirectional sync, with SHA-256 verification of existing files without downloading

## Quick Start

Install system dependencies from [docs/development.md](docs/development.md), then use the normal Cargo loop:

```bash
cargo build
cargo run
cargo test
```

`cargo build` runs `build.rs`, compiles `data/ui/*.blp` templates, and bundles UI/icon resources. Flatpak install/update, X11 visual checks, and trash-portal reproduction commands also live in [docs/development.md](docs/development.md).

## WebDAV 同步与服务器配置

**开启 WebDAV 同步前，请先在服务器侧开启 SHA-256 内容哈希功能。**
Photo Viewer 新建任务默认开启“服务器使用 SHA-256 内容 ETag”；已有任务需在设置中
手动开启同名选项。只有明确配置了该约定，应用才会将云端 ETag 作为 SHA-256 校验值。
普通 ETag、文件大小和修改时间不能证明照片内容相同。

使用 rclone WebDAV 时，在原来的启动命令中增加 `--etag-hash sha256`：

```bash
rclone serve webdav /srv/webdav --addr :443 \
  --cert /etc/webdav/tls.crt --key /etc/webdav/tls.key \
  --htpasswd /etc/webdav/htpasswd --etag-hash sha256
```

目录、证书、认证文件和其他启动参数请沿用服务器现有配置。若由 systemd 管理，修改
实际的 `ExecStart` 后执行 `sudo systemctl daemon-reload` 和
`sudo systemctl restart webdav.service`。服务重启会中断当时正在进行的请求。

可以通过下面的命令检查一张已有照片。`curl` 会提示输入 WebDAV 密码，`--head`
只读取响应头，不下载照片；也可使用 `PROPFIND` 的 `getetag` 属性读取同一个值。

```bash
curl --head --user YOUR_USERNAME 'https://YOUR_SERVER/photoviewer/Camera/example.jpg'
sha256sum /path/to/the/same/local/example.jpg
```

响应中的 `ETag` 应为带双引号的 64 位十六进制 SHA-256，并与相同本地文件的
`sha256sum` 结果一致。必须先确认服务器采用此算法，不能仅凭 ETag 长度开启该选项。
不要跳过 HTTPS 证书校验。

开启后，应用在本地读取文件计算 SHA-256，与云端内容哈希一致就建立同步基线，
**无需完整下载已有照片来校验**；不同内容仍按上传范围处理冲突或必要下载，
实际新增文件的上传和下载仍会发生。服务器也需要读取文件计算哈希，首次枚举大型
图库仍有磁盘开销，但照片内容无需为了比较而传输到客户端。

rclone 默认不带 `--etag-hash` 的 ETag 由修改时间和大小生成，不符合上述约定。
错误、缺失或弱 ETag 会在 SHA-256 模式下报告配置错误，不会假装校验成功。
不支持 SHA-256 内容 ETag 的其他 WebDAV 服务，可以关闭应用中的该选项使用兼容
模式；兼容模式首次遇到两边都有的文件可能需要完整下载校验。此选项可在任务运行中
修改，应用会安全应用新配置并自动继续，无需关闭再开启全局 WebDAV 开关。

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
