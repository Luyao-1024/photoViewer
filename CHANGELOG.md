# Changelog

## 0.5.0 (M5 complete)

- Empty states (AdwStatusPage) for all views
- Toast feedback via AdwToastOverlay
- LRU thumbnail cache cleanup (2GB limit)
- Application icon (symbolic + 64/128 PNG)
- Incremental file watcher (notify crate)
- System theme follow (AdwStyleManager)

## 0.4.0 (M4)

- EditorPage with live preview (30fps)
- 5 built-in EditOperations (Rotate/Crop/Brightness/Contrast/Saturation)
- Save Copy / Save Overwrite (mixed strategy + confirmation)
- Destructive rotation with 5s undo toast

## 0.3.0 (M3)

- AlbumsPage (folder-as-album grid)
- AlbumDetailPage (single album photos)
- TrashPage (multi-select + batch restore/delete)
- Sidebar routing (Photos/Albums/Trash)
- System trash integration with basename collision handling

## 0.2.0 (M2)

- ThumbnailLoader (worker pool + disk cache, 3 size buckets)
- ViewerPage (fullscreen + zoom/pan + keyboard nav + preloading)
- Real thumbnails replace gray placeholders

## 0.1.0 (M1)

- Project scaffold (Rust + GTK4 + Libadwaita)
- SQLite schema + migrations
- LocalBackend filesystem scanner
- EXIF DateTimeOriginal extraction
- PhotosPage with Year/Month/Day views (shared ListStore)
