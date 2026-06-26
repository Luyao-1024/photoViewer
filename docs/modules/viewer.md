# Viewer Module

## Scope

The viewer module covers the full media view, top overlay toolbar, left/right navigation, bottom thumbnail strip, video progress, details panel, and editor entry points.

## Key Files

| File | Role |
|---|---|
| `src/ui/viewer_page.rs` | Viewer state, navigation, overlay panel behavior |
| `data/ui/viewer-page.blp` | Viewer template |
| `tests/e2e_viewer.rs` | Viewer flow coverage |
| `tests/ui_viewer_toolbar.rs` | Viewer toolbar/template assertions |

## Layout Contract

The viewer is pushed inside the existing `adw::NavigationView`; it must not resize the main app sidebar. Keep viewer chrome inside the page content area and avoid constraints that alter root window/sidebar sizing.

Overlay controls should have stable dimensions. Hidden panels should not leave child content measured in a collapsed allocation path, because that can produce warnings such as negative width or height in `gtk_widget_size_allocate`.

Original image decode must apply orientation metadata before creating the display texture. Rotate from the editor changes metadata only, so the viewer must not rely on pixel dimensions from `image::open` to infer display direction.

Videos use the `GtkVideo` layer in `viewer-page.blp`, backed by `GtkMediaFile`. When switching away from a video, pause and detach the previous stream so audio/playback does not continue behind an image. The image `GtkPicture` and video `GtkVideo` are mutually exclusive for the current item.

## Thumbnail Strip

The thumbnail strip should initialize centered on the active image. If centering only happens after user interaction, the adjustment is being applied before the widget has a final allocation; schedule the centering after layout or after the thumbnail model is populated.

Filmstrip thumbnails crop with `ContentFit::Cover` inside a bounded aspect-ratio frame. Displayed thumbnails must not be more extreme than 21:9 horizontally or 9:21 vertically, and the minimum width still preserves a usable click target.

The filmstrip `ScrolledWindow` must keep `propagate-natural-width: false` and use horizontal policy `external`, not `never`. `external` hides the scrollbar while preserving a real horizontal adjustment; `never` lets the loaded thumbnail row's minimum width propagate upward and can make the viewer window grow as more thumbnails are loaded.

Thumbnail generation applies the same orientation metadata as the original viewer decode. Because the thumbnail cache key includes source mtime, orientation-only edits must update the in-memory `MediaItem.file_mtime` before refreshing the strip; waiting for the filesystem watcher leaves the current viewer session using the old cache key and can show a stale direction.

For videos, play/pause and seeking are handled by the `GtkVideo`'s own built-in media controller (its progress bar sits directly under the video). There is no separate progress widget above the filmstrip — an earlier custom `Gtk.Scale` duplicated the built-in bar and was removed.

## Navigation Buttons

Left/right image navigation belongs to viewer chrome. When positioned as top-right overlay controls, keep them visually consistent with the active material mode and avoid blocking the original image more than necessary.

## Details And Editor Panels

Details/editor side panels should be treated as overlay chrome, not as layout that changes the main image viewport unexpectedly. Collapsed state should hide or unparent expensive/size-forcing child regions where needed.

Videos are view-only. Keep the Edit button disabled for `video/*` items and guard the click handler so videos cannot configure `EditorPanel`.

The editor crop selector is drawn as a `GtkDrawingArea` overlay above the viewer `GtkPicture`. It must stay in the image overlay so users can drag the crop rectangle directly over the photo. Coordinate conversion maps the displayed contain-fitted image rectangle back to oriented source-image pixels before updating `EditorPanel`. A hit crop rectangle remains visually selected after click/drag begin so the movable/resizable affordance is obvious.

When the editor sidebar is open, the viewer overlay previous/next navigation buttons are hidden. Keyboard and gesture navigation are already blocked by `is_editing`; the visible chrome must match that locked state so editing controls are not mixed with viewer navigation.
