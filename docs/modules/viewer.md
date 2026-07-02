# Viewer Module

## Scope

The viewer module covers the full media view, top overlay toolbar, left/right navigation, bottom thumbnail strip, video progress, details panel, and editor entry points.

Viewer entry points are migrating to stable media identity. New call paths
should open a viewer with a `MediaQuery` plus `MediaId` and an initial visible
window, not with a long-lived "global index". `ViewerPage::new_for_query`
stores that query and id; left/right navigation uses
`MediaRepository::neighbor()` to find adjacent media, then syncs the local
viewer window by id. The current `ListStore` index remains an internal render
cursor only.

## Key Files

| File | Role |
|---|---|
| `src/ui/viewer_page.rs` | Viewer state, navigation, overlay panel behavior |
| `src/ui/keyboard/` | Project-wide shortcut bindings and router |
| `data/ui/viewer-page.blp` | Viewer template |
| `tests/e2e_viewer.rs` | Viewer flow coverage |
| `tests/ui_viewer_toolbar.rs` | Viewer toolbar/template assertions |

## Layout Contract

The viewer is pushed inside the existing `adw::NavigationView`; it must not resize the main app sidebar. Keep viewer chrome inside the page content area and avoid constraints that alter root window/sidebar sizing.

Overlay controls should have stable dimensions. Hidden panels should not leave child content measured in a collapsed allocation path, because that can produce warnings such as negative width or height in `gtk_widget_size_allocate`.

Original image decode must apply orientation metadata before creating the display texture. Rotate from the editor changes metadata only, so the viewer must not rely on pixel dimensions from `image::open` to infer display direction.

Videos use the `GtkVideo` layer in `viewer-page.blp`, backed by `GtkMediaFile`. When switching away from a video, pause and detach the previous stream so audio/playback does not continue behind an image. While a video stream is loading, keep the `GtkPicture` layer visible with the current video's preview thumbnail; reveal `GtkVideo` only after the stream reports `prepared` and the navigation token still matches. Outside that loading handoff, the image `GtkPicture` and video `GtkVideo` are mutually exclusive for the current item.

The image, video, and loading surfaces use the shared `viewer-media-surface`
CSS class so empty/loading backgrounds follow the current libadwaita theme.
Do not hardcode black for these surfaces; light theme must keep the stage
visually light while dark theme remains naturally subdued. `GtkVideo` renders
the playing stream through an internal `GtkPicture`; style that child picture
with the plain theme background so playback letterboxing stays white/light in
light mode instead of returning to GTK's default black or the gray stage wash.

Dynamic photos (`media_subkind=motion_photo`) open as images first. The still `GtkPicture` remains the default stage and a top-left play button starts the persisted embedded video range. Playback extracts that byte range to a temporary MP4, reuses the same `GtkVideo` layer, and switches back to the still image when the stream reports `ended`. Dynamic photos remain editable as still images; only normal `video/*` items disable editing.

Flatpak builds and Flatpak-based development runs must include `--socket=pulseaudio`. GTK/GStreamer can still render video without that sandbox permission, but audio output is unavailable, which presents as silent video playback even though the viewer code is playing the media stream.

When a video stream is created, the viewer applies the persisted video audio preferences before playback starts: `video_default_muted` controls whether newly opened videos start muted (default `true`), and `video_volume` restores the last stream volume. The settings page exposes only the default-mute switch; volume changes are persisted from the media stream itself, not from a separate settings slider.

Keep `Gtk.Video` template autoplay disabled. `show_video_stage` attaches the `GtkMediaFile`, applies the saved mute/volume state, then starts playback explicitly while the thumbnail preview remains visible; this ordering prevents `Gtk.Video`'s built-in controls/autoplay setup from overriding audio preferences at stream bind time. Volume changes reported while the stream is muted are ignored for persistence so a default-muted startup does not overwrite the last audible volume with `0.0`.

## Thumbnail Strip

The thumbnail strip should initialize centered on the active image. If centering only happens after user interaction, the adjustment is being applied before the widget has a final allocation; schedule the centering after layout or after the thumbnail model is populated.

Filmstrip thumbnails crop with `ContentFit::Cover` inside a bounded aspect-ratio frame. Displayed thumbnails must not be more extreme than 21:9 horizontally or 9:21 vertically, and the minimum width still preserves a usable click target.

The filmstrip `ScrolledWindow` must keep `propagate-natural-width: false` and use horizontal policy `external`, not `never`. `external` hides the scrollbar while preserving a real horizontal adjustment; `never` lets the loaded thumbnail row's minimum width propagate upward and can make the viewer window grow as more thumbnails are loaded.

Thumbnail generation applies the same orientation metadata as the original viewer decode. Because the thumbnail cache key includes source mtime, orientation-only edits must update the in-memory `MediaItem.file_mtime` before refreshing the strip; waiting for the filesystem watcher leaves the current viewer session using the old cache key and can show a stale direction.

For videos, play/pause and seeking are handled by the `GtkVideo`'s own built-in media controller (its progress bar sits directly under the video). There is no separate progress widget above the filmstrip — an earlier custom `Gtk.Scale` duplicated the built-in bar and was removed.

The built-in media controls are styled through viewer-scoped `GtkVideo`
internal CSS nodes, not by replacing the controller. GTK's `GtkMediaControls`
CSS node is named `controls`; keep the control bar a light translucent strip in
light mode, with a thin rounded progress trough, accent-colored played range,
and compact circular scrubber. These rules must stay scoped to
`video.viewer-media-surface` so other GTK scales and media controls are
unaffected. The `GtkVideo` widget itself must use hidden overflow so its
internal picture and control overlay clip to the same rounded
`viewer-image-frame` corners as still images. Clicking the video body, or
pressing Space while the video has focus, toggles play/pause; clicks in the
bottom built-in controls strip stay reserved for GTK's native progress and
volume controls.

## Navigation Buttons

Left/right image navigation belongs to viewer chrome. The prev/next controls float as a compact pair near the bottom-right corner over the media, lifted just above `GtkVideo`'s built-in controls so videos keep their playback and mute buttons unobstructed. Their capsule container is intentionally bare (no background) — each button draws its own glass surface only on hover/focus — so they stay light and avoid blocking the original media more than necessary.

## Switch Latency And The Deferred Switch

Left/right (and filmstrip) navigation must never show a loading animation.
The switch path is built around three guarantees:

1. **Ready-before-switch (the complete fallback).** `navigate_by_delta` does
   not call `show_at` until the target's Medium preview thumbnail is actually
   loaded. The current frame stays on screen the whole time; there is no
   `set_paintable(None)` + spinner gap. A `nav_token` is bumped on every press
   so the latest press wins and rapid presses chain; a `NAV_READY_TIMEOUT_MS`
   fallback settles the switch even if a thumbnail never arrives, so the UI
   can never get stuck on the old frame.
2. **Optimistic logical position vs. synced display.** While a switch is
   pending, `current_media_id` advances optimistically (so the next press
   computes its neighbour from the new position and rapid presses skip
   forward correctly), but `current_index` — read by the title, favorite,
   details, filmstrip, and editor — only advances when `show_at` actually
   paints. The display is always consistent with `current_index`.
3. **Neighbour prefetch warms the cache.** `prefetch_neighbors`, run from
   `show_at`, background-resolves the ±1 neighbour items (cached so the next
   press skips the DB `neighbor()` query) and warms their Medium preview
   thumbnails at `TIER_BOOST`. Combined with the 128-entry thumbnail mem
   cache, the typical switch's preview is a mem-cache hit, so the deferred
   wait is only a few milliseconds.

`show_at` itself never proactively clears the paintable or shows the spinner:
it keeps the previous frame until a new texture (preview or original) lands,
and only shows the spinner when there is genuinely nothing to display (first
viewer open). Neighbour page-cache warming (`preload_neighbor_pages`) only
`read`s the file bytes — it must not do a full `load_oriented_pixbuf`, which
used to fire two concurrent HEIC decodes per switch and steal CPU from the
current decode.

Image zoom, portable rotation, and fullscreen-preview controls sit at the image stage's top-right edge so they do not compete with the bottom-right prev/next pair. Keep their order reset, zoom-out, rotate-left, rotate-right, fullscreen, zoom-in. At identity zoom, show rotate-left, rotate-right, fullscreen, and zoom-in; reveal zoom-out and reset only once the image is enlarged, and hide the rotation buttons while enlarged. Zoom and rotation state are viewer-local display transforms, never persisted to the media file, and reset when switching media or opening the editor; videos remain view-only and do not show these image controls.

Viewer previous/next, cancel/close, video playback toggle, image transform, fullscreen-preview, details, edit, and delete shortcuts are routed through the project-wide keyboard subsystem documented in [`keyboard.md`](keyboard.md). Keep visible buttons as the primary affordance and route keyboard actions through the same viewer methods or button signal paths. Do not install touch-only pinch, pan, or global swipe controllers on the viewer image stage, because they compete with overlay buttons and keyboard-driven actions. The editor crop overlay is the exception: its direct drag interaction is part of crop editing, not viewer navigation.

## Header Toolbar

The viewer header carries four actions, left-to-right: favorite, edit, delete,
details. (The earlier add-to-album entry was removed from the
viewer — album assignment for a photo is reached from the photos grid batch
menu instead.) All viewer header buttons share one hover-only treatment: bare
at rest, glass surface on hover/focus, scoped via the `.viewer-chrome` class so
the shared `.glass-toolbar-button` rule used by other pages' headers stays
always-on.

The favorite button uses the `emblem-favorite-symbolic` heart (same glyph as the Favorites album). Favoriting does not change the button surface — it only recolors the heart icon to a translucent red (`.viewer-favorite-btn.favorite-active` color rule). The button itself never turns red.

The image-stage top-right control group also includes a fullscreen preview
action, placed between rotate-right and zoom-in. It opens a separate independent top-level `GtkWindow`,
borderless and fullscreened on the current display, with a contain-fitted
`GtkPicture` using the current viewer paintable. It must not be marked transient
for the main window, because some compositors keep transient windows at dialog
size and do not honor fullscreen. The preview window inherits the main window's
`GtkApplication` when available, uses the current monitor geometry as its
fallback default size, and must not resize/maximize the main application window,
hide viewer chrome, or change the `Adw.NavigationView` stack. Inside the preview
window, only image-stage overlay controls are recreated: the bottom-right
previous/next pair and the top-right zoom/reset/rotate/restore buttons. Header
actions, details/editor controls, and the bottom thumbnail strip do not appear
in this window. Preview zoom/rotation state is local to the fullscreen window;
preview previous/next reuses the main viewer navigation and keeps the preview
paintable synced when the main viewer image changes. Escape or the restore
button closes only that preview.

## Details And Editor Panels

Details/editor side panels should be treated as overlay chrome, not as layout that changes the main image viewport unexpectedly. Collapsed state should hide or unparent expensive/size-forcing child regions where needed.

The details panel mirrors its row set to the media kind: photos get EXIF camera-parameter rows (aperture, exposure, focal length, location, …), while videos get `ffprobe`-derived rows (duration, codec + profile, frame rate, bit rate, container, device) appended to the same `file_group` via the same dynamic-`ActionRow` mechanism. Video `width`/`height`/`taken_at` light up the shared dimensions/captured rows just like photos. Both sets load asynchronously (`load_camera_details` / `load_video_details`) behind a navigation token so switching items cancels stale loads.

The details panel name row is slightly larger than the other file rows and is activatable. Clicking it opens an inline rename entry that edits the file stem only; the original extension is preserved by the repository rename path, even if the user types a different suffix. Successful renames update the current `ListStore` item, viewer title, details rows, and filmstrip without saving any image/video pixels.

Videos are view-only. Keep the Edit button disabled for `video/*` items and guard the click handler so videos cannot configure `EditorPanel`.

The editor crop selector is drawn as a `GtkDrawingArea` overlay above the viewer `GtkPicture`. It must stay in the image overlay so users can drag the crop rectangle directly over the photo. Coordinate conversion maps the displayed contain-fitted image rectangle back to oriented source-image pixels before updating `EditorPanel`. A hit crop rectangle remains visually selected after click/drag begin so the movable/resizable affordance is obvious.

When the editor sidebar is open, the viewer overlay previous/next navigation buttons are hidden. Keyboard navigation is blocked by the `Editor` keyboard scope; the visible chrome must match that locked state so editing controls are not mixed with viewer navigation.
