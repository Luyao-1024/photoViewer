# Viewer Module

## Scope

The viewer module covers the full media view, top overlay toolbar, left/right navigation, bottom thumbnail strip, video progress, details panel, and editor entry points.

Viewer entry points are migrating to stable media identity. New call paths
should open a viewer with a `MediaQuery` plus `MediaId` and an initial visible
window, not with a long-lived "global index". During migration,
`ViewerPage::new_for_query` resolves the id inside the current window and then
uses the existing index-based internals; future navigation work should ask the
repository for neighboring items when the viewer moves beyond the loaded
window.

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

Dynamic photos (`media_subkind=motion_photo`) open as images first. The still `GtkPicture` remains the default stage and a top-left play button starts the persisted embedded video range. Playback extracts that byte range to a temporary MP4, reuses the same `GtkVideo` layer, and switches back to the still image when the stream reports `ended`. Dynamic photos remain editable as still images; only normal `video/*` items disable editing.

Flatpak builds and Flatpak-based development runs must include `--socket=pulseaudio`. GTK/GStreamer can still render video without that sandbox permission, but audio output is unavailable, which presents as silent video playback even though the viewer code is playing the media stream.

When a video stream is created, the viewer applies the persisted video audio preferences before playback starts: `video_default_muted` controls whether newly opened videos start muted (default `true`), and `video_volume` restores the last stream volume. The settings page exposes only the default-mute switch; volume changes are persisted from the media stream itself, not from a separate settings slider.

Keep `Gtk.Video` template autoplay disabled. `show_video_stage` attaches the `GtkMediaFile`, applies the saved mute/volume state, then starts playback explicitly; this ordering prevents `Gtk.Video`'s built-in controls/autoplay setup from overriding audio preferences at stream bind time. Volume changes reported while the stream is muted are ignored for persistence so a default-muted startup does not overwrite the last audible volume with `0.0`.

## Thumbnail Strip

The thumbnail strip should initialize centered on the active image. If centering only happens after user interaction, the adjustment is being applied before the widget has a final allocation; schedule the centering after layout or after the thumbnail model is populated.

Filmstrip thumbnails crop with `ContentFit::Cover` inside a bounded aspect-ratio frame. Displayed thumbnails must not be more extreme than 21:9 horizontally or 9:21 vertically, and the minimum width still preserves a usable click target.

The filmstrip `ScrolledWindow` must keep `propagate-natural-width: false` and use horizontal policy `external`, not `never`. `external` hides the scrollbar while preserving a real horizontal adjustment; `never` lets the loaded thumbnail row's minimum width propagate upward and can make the viewer window grow as more thumbnails are loaded.

Thumbnail generation applies the same orientation metadata as the original viewer decode. Because the thumbnail cache key includes source mtime, orientation-only edits must update the in-memory `MediaItem.file_mtime` before refreshing the strip; waiting for the filesystem watcher leaves the current viewer session using the old cache key and can show a stale direction.

For videos, play/pause and seeking are handled by the `GtkVideo`'s own built-in media controller (its progress bar sits directly under the video). There is no separate progress widget above the filmstrip — an earlier custom `Gtk.Scale` duplicated the built-in bar and was removed.

## Navigation Buttons

Left/right image navigation belongs to viewer chrome. The prev/next controls float as a compact pair near the bottom-right corner over the media, lifted just above `GtkVideo`'s built-in controls so videos keep their playback and mute buttons unobstructed. Their capsule container is intentionally bare (no background) — each button draws its own glass surface only on hover/focus — so they stay light and avoid blocking the original media more than necessary.

Image zoom controls sit at the image stage's top-right edge so they do not compete with the bottom-right prev/next pair. Keep their order reset, zoom-out, zoom-in. At identity zoom, show only the zoom-in button; reveal zoom-out and reset only once the image is enlarged. Zoom state is viewer-local and resets when switching media, opening the editor, or using the reset button; videos remain view-only and do not show the image zoom controls.

## Header Toolbar

The viewer header carries four actions, left-to-right: favorite, edit, delete, details. (The earlier add-to-album entry was removed from the viewer — album assignment for a photo is reached from the photos grid batch menu instead.) All viewer header buttons share one hover-only treatment: bare at rest, glass surface on hover/focus, scoped via the `.viewer-chrome` class so the shared `.glass-toolbar-button` rule used by other pages' headers stays always-on.

The favorite button uses the `emblem-favorite-symbolic` heart (same glyph as the Favorites album). Favoriting does not change the button surface — it only recolors the heart icon to a translucent red (`.viewer-favorite-btn.favorite-active` color rule). The button itself never turns red.

## Details And Editor Panels

Details/editor side panels should be treated as overlay chrome, not as layout that changes the main image viewport unexpectedly. Collapsed state should hide or unparent expensive/size-forcing child regions where needed.

The details panel mirrors its row set to the media kind: photos get EXIF camera-parameter rows (aperture, exposure, focal length, location, …), while videos get `ffprobe`-derived rows (duration, codec + profile, frame rate, bit rate, container, device) appended to the same `file_group` via the same dynamic-`ActionRow` mechanism. Video `width`/`height`/`taken_at` light up the shared dimensions/captured rows just like photos. Both sets load asynchronously (`load_camera_details` / `load_video_details`) behind a navigation token so switching items cancels stale loads.

Videos are view-only. Keep the Edit button disabled for `video/*` items and guard the click handler so videos cannot configure `EditorPanel`.

The editor crop selector is drawn as a `GtkDrawingArea` overlay above the viewer `GtkPicture`. It must stay in the image overlay so users can drag the crop rectangle directly over the photo. Coordinate conversion maps the displayed contain-fitted image rectangle back to oriented source-image pixels before updating `EditorPanel`. A hit crop rectangle remains visually selected after click/drag begin so the movable/resizable affordance is obvious.

When the editor sidebar is open, the viewer overlay previous/next navigation buttons are hidden. Keyboard and gesture navigation are already blocked by `is_editing`; the visible chrome must match that locked state so editing controls are not mixed with viewer navigation.
