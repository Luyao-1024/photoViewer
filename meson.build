project('photo-viewer', 'rust',
  version : '0.5.0',
  meson_version : '>= 0.59.0',
)

# V1: only package the desktop file. Blueprint compilation is handled by
# build.rs via glib_build_tools; full Blueprint integration arrives in M2.

install_data(
  'data/org.gnome.PhotoViewer.desktop',
  install_dir : get_option('datadir') / 'applications',
)

install_data(
  'data/org.gnome.PhotoViewer.metainfo.xml',
  install_dir : get_option('datadir') / 'metainfo',
)

# Install application icons into the hicolor theme so the desktop entry
# (Icon=org.gnome.PhotoViewer) resolves under standard XDG icon lookups
# and GNOME shell taskbar grouping (StartupWMClass=Photo Viewer) finds
# the right icon.
icondir = join_paths(get_option('datadir'), 'icons', 'hicolor')
install_data(
  'data/icons/photo-viewer-64.png',
  install_dir : icondir / '64x64' / 'apps',
  rename : 'org.gnome.PhotoViewer.png',
)
install_data(
  'data/icons/photo-viewer-128.png',
  install_dir : icondir / '128x128' / 'apps',
  rename : 'org.gnome.PhotoViewer.png',
)
install_data(
  'data/icons/photo-viewer-symbolic.svg',
  install_dir : icondir / 'symbolic' / 'apps',
  rename : 'org.gnome.PhotoViewer-symbolic.svg',
)
