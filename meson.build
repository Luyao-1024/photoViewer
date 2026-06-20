project('photo-viewer', 'rust',
  version : '0.1.0',
  meson_version : '>= 0.59.0',
)

# V1: only package the desktop file. Blueprint compilation is handled by
# build.rs via glib_build_tools; full Blueprint integration arrives in M2.

install_data(
  'data/org.gnome.PhotoViewer.desktop',
  install_dir : get_option('datadir') / 'applications',
)