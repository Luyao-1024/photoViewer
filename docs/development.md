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

For current-worktree debug runs:

```bash
cargo build
flatpak run \
  --filesystem=/home/luyao/workspace/photo_viewer/photoViewer \
  --filesystem=home \
  --command=sh org.gnome.PhotoViewer \
  -c 'exec /home/luyao/workspace/photo_viewer/photoViewer/target/debug/photo-viewer'
```

For reinstalling the latest app:

```bash
flatpak-builder --user --install --ccache --disable-rofiles-fuse --force-clean \
  /tmp/photoViewer-flatpak-build org.gnome.PhotoViewer.yml
```

Avoid using repository-local `.flatpak-builder` state as a routine install path while the known `rofiles-fuse` unmount hang is present.

## Documentation Workflow

- Put module-specific behavior in `docs/modules/`.
- Keep root `AGENTS.md` as an index and workflow guide only.
- When changing a module contract, update the matching module doc in the same change.
- Historical plans/specs under `docs/superpowers/` should not become the primary source for current behavior.
