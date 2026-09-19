# Development

## System Dependencies

Fedora:

```bash
sudo dnf install gtk4-devel libadwaita-devel gdk-pixbuf2-devel \
                 libheif-devel sqlite-devel blueprint-compiler \
                 at-spi2-core dbus-daemon python3-pyatspi
```

Ubuntu:

```bash
sudo apt install libgtk-4-dev libadwaita-1-dev libgdk-pixbuf-2.0-dev \
                 libheif-dev libsqlite3-dev at-spi2-core dbus-daemon \
                 libdbus-1-dev python3-pyatspi
```

`blueprint-compiler` must be available on `PATH`.

## Build And Run

```bash
cargo build
./run-flatpak.sh
```

`cargo build` runs `build.rs`, which compiles `data/ui/*.blp` to generated
`.ui` files under `OUT_DIR` and bundles them as resources without writing into
`data/ui/`. `meson.build` is for install-time desktop integration. Use
`./run-flatpak.sh` as the canonical interactive startup entry point: it builds
in the GNOME SDK but launches through the installed app sandbox, preserving the
real thumbnail/cache/GIO behavior. `cargo run` remains useful for a narrow
local debugger session, but it does not share the Flatpak cache or reproduce
sandbox integration.

The runner is also the single entry point for launch variants:

```bash
./run-flatpak.sh                 # debug build, normal app run
./run-flatpak.sh -r              # production-like release run
./run-flatpak.sh -T startup,scan # focused reusable performance trace
./run-flatpak.sh -t              # legacy whole-process Perfetto trace
./run-flatpak.sh -L storage=debug
```

`-T` accepts `startup`, `database`, `scan`, `filesystem`, `thumbnail`, `mutation`, or `all`, and may be repeated. See `docs/modules/diagnostics.md` for the capture contract and Perfetto analysis workflow.

While changes are uncommitted, run only the new or directly modified focused
tests. Before pushing to a remote, follow the full CI gate and commit-message
recording policy in [`docs/testing.md`](testing.md). A local handoff that is not
being pushed needs only an honest record of its focused verification.

### WebDAV and Flatpak

WebDAV requires the manifest's `--share=network` permission. Persistent passwords use the desktop Secret Service through `org.freedesktop.secrets`; host `cargo run` and the installed Flatpak can therefore exercise different keyring/session-bus environments. Reinstall the Flatpak after changing either permission, and test connection creation from inside the sandbox before claiming service compatibility.

The manifest builds Cargo offline from `cargo-sources.json`. After changing Rust dependencies or `Cargo.lock`, regenerate that file with the official `flatpak-cargo-generator.py` and verify a locked Flatpak build. Do not commit a manifest that references new crates while leaving the offline source list stale.

Real-service test credentials and WebDAV URLs must not be committed. Configure them through the Settings page or an isolated local test environment. A production-like acceptance pass should cover initial upload/download, changes from both sides, conditional conflict handling, restart recovery, Unicode paths, and the system keyring from the installed Flatpak.

### Large-library benchmark

The opt-in benchmark creates a disposable SQLite library and records initial
page, deep OFFSET page, name search, and unchanged-scan snapshot timings. It is
ignored by normal CI so benchmark size and machine noise cannot make correctness
checks flaky.

```bash
PHOTOVIEWER_BENCH_ITEMS=100000 \
  cargo test --release --test library_benchmark -- --ignored --nocapture
```

Use the same item count, release profile, filesystem, and machine when comparing
results. Supported sizes are clamped to 1,000 through 1,000,000 rows.

## Flatpak Visual Checks

Liquid Glass depends on GTK runtime support for `backdrop-filter`. The host GTK may be older than the target runtime, so visual checks for blur/refraction-style surfaces should run through the Flatpak GNOME 50 runtime.

Automated visual smoke screenshots are supported only on X11. Wayland
compositors do not expose a common screenshot/control interface, so the project
skips automated visual checks on Wayland instead of depending on
compositor-specific tools.

Additional Fedora dependencies:

```bash
sudo dnf install xorg-x11-server-Xvfb xdotool ImageMagick xorg-x11-utils
```

Additional Ubuntu dependencies:

```bash
sudo apt install xvfb xdotool imagemagick x11-utils python3-pyatspi
```

Run the X11 visual smoke check:

```bash
tools/visual-check-x11.sh
```

The script uses the current X11 display when available. In non-Wayland
headless environments it starts `Xvfb`, launches the app through
`run-flatpak.sh`, waits for the window, and writes a screenshot to
`target/visual-checks/`. If `XDG_SESSION_TYPE=wayland`, it prints a skip
message and exits successfully.

To verify a non-destructive real-input path as well as startup rendering, run:

```bash
tools/visual-check-x11.sh --keyboard-smoke
```

This sends `Ctrl+F` to the Flatpak window over X11 and captures both the
startup screen and the resulting Search page. The check fails if the window
does not visibly change. It is intentionally limited to navigation and does
not alter library data. In headless runs it also starts a private session
D-Bus and verifies the AT-SPI bus plus registry before GTK starts, so the
Flatpak app can connect to its accessibility backend. The manifest and
development runner both grant the narrowly scoped `org.a11y.Bus` permission;
reinstall the Flatpak after changing the manifest for installed-app runs.

To additionally verify the controls a screen reader receives, run:

```bash
tools/visual-check-x11.sh --a11y-smoke
```

This implies the non-destructive `Ctrl+F` keyboard smoke test, then uses
`python3-pyatspi` to assert the running Flatpak exposes its localized
application frame (currently `照片查看器`), a focused Search entry,
and the `全部`、`文件名`、`日期` search-field toggle buttons. It is an X11/Xvfb
check and complements, rather than replaces, manual screen-reader testing on a
desktop session.

For current-worktree debug runs:

```bash
cargo build
flatpak run \
  --filesystem=/home/luyao/workspace/photo_viewer/photoViewer \
  --filesystem=home \
  --command=sh io.github.luyao_1024.photoviewer \
  -c 'exec /home/luyao/workspace/photo_viewer/photoViewer/target/debug/photo-viewer'
```

For reinstalling the latest app:

```bash
flatpak-builder --user --install --ccache --disable-rofiles-fuse --force-clean \
  /tmp/photoViewer-flatpak-build io.github.luyao_1024.photoviewer.yml
```

Avoid using repository-local `.flatpak-builder` state as a routine install path while the known `rofiles-fuse` unmount hang is present.

Video thumbnails in the installed Flatpak depend on the bundled
`ffmpegthumbnailer` binary. Its shared library must install under `/app/lib`
because the Flatpak runtime loader searches `/app/lib`, not `/app/lib64`.

## Flatpak Trash Portal Check

Trash failures must be reproduced from the app sandbox, not by running host
`gio trash`, because host GIO bypasses the Flatpak Trash portal. Use:

```bash
tools/flatpak-trash-portal-repro.sh
```

The script creates a temporary file under `~/Pictures`, logs the installed app
permissions, runs `gio info` and `gio trash` via
`flatpak run --command=gio io.github.luyao_1024.photoviewer`, then cleans up the
test file or trash entry.

## Documentation Workflow

- Put module-specific behavior in `docs/modules/`; each module doc should own its current contracts, key files, and known pitfalls.
- Keep root `README.md`, `CONTRIBUTING.md`, and `AGENTS.md` short. They should point to `docs/README.md` or a specific module doc instead of repeating long command blocks or architecture details.
- Keep `docs/README.md` as the canonical documentation index. Add new module docs there and in `AGENTS.md` when they become required reading for code changes.
- When changing a module contract, UI invariant, or development workflow, update the matching module/core doc in the same change.
- Historical plans/specs under `docs/superpowers/` are archived context. Do not update them as the primary source for current behavior; migrate lasting rules into the relevant maintained doc.
- Prefer replacing duplicate detailed documents with a short compatibility pointer to the active source of truth.
