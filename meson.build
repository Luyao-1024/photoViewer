project('photo-viewer', 'rust',
  version : '0.1.0',
  meson_version : '>= 0.59.0',
)

gnome = import('gnome')

# V1: only schema and desktop placeholder
# Full Blueprint integration arrives in M2

install_data(
  'data/org.gnome.PhotoViewer.desktop',
  install_dir : get_option('datadir') / 'applications',
)