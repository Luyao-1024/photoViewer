# Development

## System Dependencies

Fedora:

```bash
sudo dnf install gtk4-devel libadwaita-devel gdk-pixbuf2-devel \
                 libheif-devel sqlite-devel blueprint-compiler
```

Ubuntu:

```bash
sudo apt install libgtk-4-dev libadwaita-1-dev libgdk-pixbuf-2.0-dev \
                 libheif-dev libsqlite3-dev
```

`blueprint-compiler` must be available on `PATH`.

## Build And Run

```bash
cargo build
cargo run
```

`cargo build` runs `build.rs`, which compiles `data/ui/*.blp` to `.ui` and bundles resources. `meson.build` is for install-time desktop integration; the normal inner loop is Cargo.

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
sudo apt install xvfb xdotool imagemagick x11-utils
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

## Documentation Workflow

- Put module-specific behavior in `docs/modules/`.
- Keep root `AGENTS.md` as an index and workflow guide only.
- When changing a module contract, update the matching module doc in the same change.
- Historical plans/specs under `docs/superpowers/` should not become the primary source for current behavior.
