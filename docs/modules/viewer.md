# Viewer Module

## Scope

The viewer module covers the full media view, top overlay toolbar, left/right navigation, bottom thumbnail strip, video progress, details panel, and editor entry points.

Viewer entry points are migrating to stable media identity. New call paths
should open a viewer with a `MediaQuery` plus `MediaId` and an initial visible
window, not with a long-lived "global index". `ViewerPage::new_for_query`
stores that query and id; left/right navigation uses
`MediaRepository::neighbor()` to find adjacent media, then syncs the local
viewer window by id. The repository must resolve neighbours through
query-specific SQL projections for live media, search/search-kind results,
folder albums, favorites, image/video type albums, and motion photos; do not
load the full query result just to move one step in the viewer. The current
`ListStore` index remains an internal render cursor only.

## Key Files

| File | Role |
|---|---|
| `src/ui/viewer_page.rs` | Viewer state, navigation, overlay panel behavior |
| `src/ui/viewer/actions.rs` | Viewer toolbar delete/favorite actions, favorite state sync, undo of the move to trash, and post-delete navigation |
| `src/ui/viewer/transform.rs` | Image-stage zoom/rotation controls, CSS transform updates, and transform math helpers |
| `src/ui/viewer/fullscreen_window.rs` | Independent fullscreen preview window, preview navigation buttons, and preview-local transforms |
| `src/ui/viewer/immersive.rs` | In-place immersive browsing: the stillness timer, the four chrome revealers, and the two refusals |
| `src/ui/viewer/filmstrip.rs` | Filmstrip geometry plus stateful UI wiring, bounded window rebuild/extend, thumbnail buttons, and centering animation |
| `src/ui/viewer/details.rs` | Details panel wiring, EXIF/video row population, and metadata formatting helpers |
| `src/ui/viewer/navigation.rs` | Stable-id lookup, viewer navigation actions, deferred switch, neighbour cache, and prefetch helpers |
| `src/ui/viewer/stage.rs` | Image/video stage loading, animated/motion playback, video stream lifecycle, and playback helper predicates |
| `src/ui/viewer/fullscreen.rs` | Shared fullscreen/overlay button helper used by viewer chrome |
| `src/ui/viewer/crop.rs` | Editor crop overlay drawing, hit-testing, drag/resize geometry, and overlay-to-source coordinate conversion |
| `src/ui/viewer/editor.rs` | Editor side-panel lifecycle, editor callback wiring, save-result dialogs, and editor navigation lock state |
| `src/ui/keyboard/` | Project-wide shortcut bindings and router |
| `src/ui/toasts.rs` | Toast priorities/timeouts and the one inline Undo action |
| `src/ui/media_list.rs` | Shared live-list helpers, including the sorted re-insert an undo relies on |
| `data/ui/viewer-page.blp` | Viewer template |
| `tests/pipeline_thumbnails.rs` | Thumbnail pipeline coverage (no viewer window) |
| `tests/ux_viewer_pointer_flows.rs` | Real-pointer viewer UX runs: hit test, press, assert the switch |
| `tests/ui_viewer_toolbar.rs` | Viewer toolbar/template assertions |
| `tests/ui_viewer_source_structure.rs` | Display-free viewer source/template invariants |

Viewer unit tests live beside the behavior they cover. Production source files
declare `#[cfg(test)] mod tests;`, and test bodies live in child test files such
as `src/ui/viewer_page/tests.rs`, `src/ui/viewer/filmstrip/tests.rs`, and
`src/ui/viewer/stage/tests.rs`.

## Layout Contract

The viewer is pushed inside the existing `adw::NavigationView`; it must not resize the main app sidebar. Keep viewer chrome inside the page content area and avoid constraints that alter root window/sidebar sizing.

The viewer header shows the same cloud/cloud-off state beside the date for
images and videos in selected sync albums. It queries the current media ID off
the GTK thread on each switch and on sync-state domain events, and accepts the
result only while both the switch and latest badge-request tokens still match.
Refreshing the same item retains its previous glyph until the new state lands,
so transfer events do not flash a blank badge. Media outside selected albums
have no cloud glyph. The WebDAV Day-view cloud-icon switch does not affect this
header indicator.

Neither glyph in this row comes from the icon theme — the heart from
`src/ui/favorite_icon.rs`, the cloud from the app's own bitmaps — so neither
inherits a size the way a theme icon would. A bare `GtkImage` takes GTK's
16 px default, while `favorite_btn`'s icon takes whatever the header resolves,
and those are not the same number in every theme or libadwaita version. When that happened the badge rendered a third smaller
than the heart on the other side of the same bar. `setup_favorite_button`
therefore reads the resolved icon size off the favorite button's image and
copies it onto the badge on every map (`sync_badge_to_toolbar_icon_size`).
Neither the template nor `.viewer-sync-badge` may pin a size, or it overrides
the copy; `tests/ui_viewer_badge_size.rs` pins the behaviour by rendering the
header against a toolbar that resolves 20 px. The artwork half of the same
contract (hairline stroke, a silhouette that fills its box, identical geometry
across states and themes) lives with the badge in `src/ui/cloud_badge.rs` and is
documented in [docs/modules/sync.md](sync.md).

Overlay controls should have stable dimensions. Hidden panels should not leave child content measured in a collapsed allocation path, because that can produce warnings such as negative width or height in `gtk_widget_size_allocate`.

Original image decode must apply orientation metadata before creating the display texture. Rotate from the editor changes metadata only, so the viewer must not rely on pixel dimensions from `image::open` to infer display direction.

Animated images open on the image stage, not the video stage. The scanner persists `media_attributes.animated` for known animated sources, but the viewer also probes the current file's GIF header when opening a single image so stale DB rows or GIF content with a `.jpg` suffix can still play. Grid and filmstrip thumbnails remain static previews. Viewer playback starts automatically, loops, and holds the last frame for an extra 500 ms before starting the next loop. GIF decoding runs off the GTK thread and is capped at 300 frames and 64 Mi decoded RGBA pixels; media beyond either safety budget falls back without retaining a partial full-frame animation in memory.

Videos use the `GtkVideo` layer in `viewer-page.blp`, backed by `GtkMediaFile`. When switching away from a video, pause and detach the previous stream so audio/playback does not continue behind an image. Detach is immediate (the picture switches away and audio stops), but the detached `GtkMediaFile` is then retained in `retired_video_stream` for one idle cycle before its last reference is released: dropping it synchronously finalized the stream while its GstPlay thread was still emitting state-changed signals, crashing that thread with a use-after-free inside libgobject signal dispatch. `show_at` must also skip the video-stage rebuild when the resolved item is the same media id as the stream currently attached to `GtkVideo` — the startup scan re-anchors the render index onto the same item when media is inserted before it, and rebuilding there would destroy a live GstPlay mid-flight for no reason. Do not base that reuse decision on `current_media_id`, because left/right navigation advances it optimistically before the deferred visual switch paints. While a video stream is loading, keep the `GtkPicture` layer visible with the current video's preview thumbnail; reveal `GtkVideo` only after the stream reports `prepared` and the navigation token still matches. Outside that loading handoff, the image `GtkPicture` and video `GtkVideo` are mutually exclusive for the current item.

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

The viewer owns one `.viewer-media-error` surface (`media_error_box`) for media
that cannot be shown, and it replaces the stage rather than covering it. When a
normal video stream reports an error, hide `GtkVideo` and the preview picture
and show that surface, which keeps unsupported codecs and damaged files from
exposing GTK/GStreamer's default broken-frame graphic. Motion-photo playback
errors are different: they restore the still image rather than showing the error
surface.

A failed original decode uses the same surface, but only when there is nothing
to paint: a warm preview thumbnail already shows the picture, and overlaying
"cannot be displayed" on a visible image would contradict the user. `show_media_error`
picks the wording by media kind, and for images the body names the file, because
a moved or renamed file is the case the user can actually go fix. The surface
offers 重试 (which re-enters `show_at` for the current index) and 在文件管理器中显示
(which opens the containing folder through `gtk::show_uri_full` and toasts when no
handler accepts it). The error surface lives in `base.css` only — it is a flat
themed wash, not a glass material — so it needs no liquid/plain mirror.

Both buttons are the only way off a photo that will not decode, so both are
covered end to end by `journey_corrupt_photo_offers_retry_and_reveal`, against a
file that really is corrupt (`Shell::seed_broken_photo` writes JPEG magic bytes
and no image). That fixture choice matters: the failure has to be in the *file*
for this path to be the thing under test — a missing row never reaches the
viewer, and `show_original_decode_error`'s "only when there is nothing to paint"
guard means a file that decoded at thumbnail size would correctly suppress the
surface.

Retry is pressed, and the error surface is asserted to come *back*: the file is
still corrupt, so the honest outcome is the same error again. That is exactly
what used not to happen. `thumbnails::decode` answers a failed decode with a
generated "unavailable" stand-in rather than an error, so the picture had a
paintable, and `show_original_decode_error` — which reads *any* paintable as
"the picture is showing" — declined to raise the surface. The user was left on a
grey box with neither Retry nor Reveal on it, and no number of presses could get
them back. The stand-in is now flagged (`LoadedThumb::unavailable`, set from
`DecodeOrigin::Unavailable`) and the stage raises the error surface instead of
painting it, so an image that cannot be decoded behaves like a video that cannot
be played. Grid and album tiles still draw the stand-in: a grey cell is a fair
signal there, and they have no error surface to offer.

"Show in File Manager" is pressed too, and what it hands over is captured by
`set_reveal_folder_observer_for_tests`. That seam **replaces** the platform call
rather than watching it: `gtk::show_uri_full` launches the desktop's file manager
at the fixture directory, which the shell deletes moments later, so a run that let
the real handoff through leaves a window behind that outlives it and reports the
folder as missing. A test must not reach into someone's session to do that. What
the application decides — the containing folder, or the file's own URI when it has
no parent — is asserted against a fixed fixture directory, and the desktop stays
out of it. Without an observer installed the real handoff is unchanged.

## Feedback Toasts And Undo

The viewer is the only page with a toast host, and `Adw.ToastOverlay` wraps the
image stage inside `content_box` — never the page root. An overlay that spans
the whole viewer parks its bottom toast on the filmstrip, which is the control
the user is still operating while the toast reports what they just did there.
`src/ui/viewer_page/tests.rs::a_toast_lands_above_the_filmstrip` measures the
painted bounds of a real toast against the strip, so re-widening the overlay
fails a test instead of a review. Note that `allocation()` is not usable for
that check: the toast widget's allocation includes the theme margin around the
card, and a child of `Adw.ToastOverlay` can report an offset different from the
one it paints at. Use `compute_bounds`, and do not try to lift the card with CSS
- `margin-bottom` on the internal toast node grows the node rather than moving
the card.

A toast button is the app's only inline affordance, so it is reserved for
actions that can genuinely be undone from the toast (`toasts::success_with_action`,
6 s so the button is reachable). Both viewer toolbar mutations qualify:

- Favorite re-applies the previous state. The undo itself re-enters with
  `announce = false`, because a rollback that offers another rollback is a toast
  chain, not an undo.
- Move to trash runs `restore_deleted_item`, which restores the *file* first
  (`MediaRepository::restore_batch` moves it out of the trash root and clears the
  DB mark through the actor, which is what emits the domain events other pages
  listen to), and only then re-inserts the row into the live list with
  `media_list::insert_media_item_sorted`, so a failed undo cannot leave a tile
  pointing at a missing photo. Writing straight to the pool would fix this view
  and desynchronise the rest of the library. The viewer then shows the restored
  photo again.

Batch favorite elsewhere (Photos selection, search results) deliberately has no
undo toast: the action is a toggle the user can reverse with the same menu item,
and those pages have no toast host.

## Thumbnail Strip

The thumbnail strip is a low raised-glass carousel surface and should initialize centered on the active image. If centering only happens after user interaction, the adjustment is being applied before the widget has a final allocation; schedule the centering after layout or after the thumbnail model is populated.

**The invariant, and how it is checked.** The current photo's thumbnail is in
the middle of the filmstrip in every scenario — on open, on Next, on the arrow
keys, on a jump to the last photo, and on a jump back to the first. That is what
makes "keep going" and "go back" legible in a long run, and it is easy to lose at
both ends of the list, where there is nothing to scroll *toward*.

Measuring it needs the transform. The strip sizes its content to the viewport
and centres with `apply_thumb_strip_transform`, so `upper == page_size` and the
horizontal adjustment never moves at all; reading the adjustment reports a
perfectly centred strip as "no scroll happened". `journey_filmstrip_shows_every_photo_and_centres_the_current_one`
measures `compute_bounds` in the window's coordinate space instead, which carries
the transform and therefore reports what is on screen. Several guards in
`update_thumb_scroll_position` return `false` quietly — no page size, current
before the window, a missing offset, an unallocated button, unstable widths — so
a regression shows up as a silently off-centre strip rather than a failure, which
is why this needs an end-to-end scenario rather than the geometry unit tests.

**Centring settles by convergence, not by a fixed frame count.** Centring reads
each thumbnail's `allocation().width()` and re-adds them up, so it is only
correct once those allocations are final — and a filmstrip re-measures itself
every time one of its thumbnails decodes. Two things had to change for that to
hold:

- The tick loop stops when a pass both applied a transform *and* left it where
  the previous pass did (`should_retry_thumb_centering`). Stopping at the first
  pass that merely *applied* something freezes the strip against a layout that
  was about to move: a thumbnail whose `width-request` was already correct still
  grew from 39px to 55px when its texture landed, and the strip never revisited
  it, leaving the current photo 8px off centre for as long as the viewer stayed
  open.
- A centring request that arrives *while* a burst is running is recorded, not
  dropped (`thumb_scroll_requested`). The old duplicate-suppression threw it
  away, so a thumbnail that finished loading mid-burst lost its re-centring
  entirely. `THUMB_CENTER_RETRY_FRAMES` stays a ceiling on a pathological run
  and has to be comfortably larger than the frames a filmstrip needs to finish
  loading, or a burst can expire with a request still waiting on it.

`clamp_thumb_residual` bounds the residual by the scroll range, but the residual
is a pure visual nudge on top of wherever the scroll landed, so it is also bounded
by half a page. Bounding it by the scroll range alone made centring impossible
in the case that matters most: a strip whose content overflows the viewport by a
little reaches the end of its scroll while the current photo is still far from
centre, and no legal residual can make up the difference.

A filmstrip of mixed aspect ratios is what exposed most of this, and it is the
fixture set to keep. Equal shapes make several of these conditions unreachable:
"the current thumbnail is the widest" only holds when every thumbnail has the
same shape, and centring errors that accumulate with the offset of the current
item are invisible when every item lays out identically.

Filmstrip thumbnails crop with `ContentFit::Cover` inside a bounded aspect-ratio frame. Displayed thumbnails must not be more extreme than 21:9 horizontally or 9:21 vertically, and the minimum width still preserves a usable click target.

When media dimensions are available, the filmstrip button should reserve the
final clamped thumbnail width before the async thumbnail texture arrives. Do
not start every item at the minimum placeholder width and then resize on load;
that changes total carousel width after first paint and causes a visible
recentering twitch.

The active filmstrip thumbnail may be emphasized with transform, outline,
shadow, opacity, or background paint only. It must not change margins, padding,
minimum size, or any other layout-affecting property; otherwise the selected
state pushes neighboring thumbnails and creates visible strip jitter while
navigation moves.

The filmstrip content keeps a fixed internal edge inset so thumbnails do not
touch the rounded carousel edges while navigation or scroll animation is in
progress. Keep the CSS `.viewer-thumb-strip` horizontal padding and the Rust
`THUMB_EDGE_INSET` geometry constant in sync.

Filmstrip edge washes and container shadow live in the Liquid/Plain material
files, so the material switch and transparency slider affect them too. The
shared base stylesheet owns geometry and active-thumbnail emphasis only.

The filmstrip `ScrolledWindow` must keep `propagate-natural-width: false` and use horizontal policy `external`, not `never`. `external` hides the scrollbar while preserving a real horizontal adjustment; `never` lets the loaded thumbnail row's minimum width propagate upward and can make the viewer window grow as more thumbnails are loaded.

The carousel keeps only a bounded live GTK widget window around the current region. Reaching `THUMB_WINDOW_MAX` must not stop loading later or earlier thumbnails; extending at the cap slides the `[start, end)` window in the requested direction and trims the opposite edge. This keeps large libraries feeling continuous without letting the filmstrip accumulate unbounded buttons.

When the thumbnail window grows without sliding, update it incrementally:
append right-side items or prepend left-side items. Do not rebuild the whole
strip for ordinary lazy growth, because GTK briefly reallocates every recreated
child to tiny sizes and the selected thumbnail can visibly flash. Full rebuilds
are reserved for initial load, current index outside the live window, and
sliding windows where the existing indices genuinely change.

After rebuilding or extending the carousel, do not compute centering from
transient tiny child allocations. GTK can briefly report near-zero widths before
the new buttons settle; centering should retry on later frames and coalesce
bursts of thumbnail-loaded callbacks into a single pending scroll update.

When the carousel has a real horizontal adjustment range, moving the active
thumbnail into view should animate the adjustment over a short ease-out window
instead of jumping directly to the target value. A newer navigation target must
cancel the previous animation and continue from the current adjustment value, so
rapid key presses feel continuous rather than queued.

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

Reaching the head or the tail of the query must say so on the pair itself, not as silent a no-op. The arrow that has nowhere to go is dimmed (`set_sensitive(false)`, with the disabled visual carried in `base.css` as `.viewer-overlay-nav-btn:disabled { opacity: 0.32 }` — opacity is the only channel left because that selector already pins `color: #ffffff`). The dimmed-but-still-present state survives a switch: `show_at` calls `reset_nav_bounds()` so a different item is unknown until its own `prefetch_neighbors` resolves, then both directions are re-evaluated against the actual query. `preload_neighbor_pages` and `prefetch_neighbors` run before the image and video stage branches split, so the re-anchor early return for a re-shown video still reaches them — a video at either end of the query must dim its arrow on open, not after the first navigation. Keyboard `←/→` at a resolved end returns `KeyboardResult::Ignored` (via `viewer/navigation.rs::handle_nav_key`), so the press is not silently consumed while there is nothing on screen to act on. While the editor's own fields have focus, `←/→` stay `Handled` so they do not leak into the underlying grid.

### Overlay chrome must never be fill-aligned

`nav_buttons_revealer` and `zoom_controls_revealer` are `Gtk.Overlay` children
of `image_overlay`, and they must carry the **same non-filling alignment as the
cluster they wrap** (`halign: end` plus `valign: end` / `valign: start`). Do not
drop those properties and let them default to `fill`.

`Gtk.Overlay` allocates every non-main child the whole overlay area, and GTK's
hit test falls back to a windowless child whenever the pointer is not inside that
child's own content. A fill-aligned Revealer around a small button cluster is
therefore the target for *every* point on the stage. Because
`zoom_controls_revealer` is declared after `nav_buttons_revealer`, it wins that
fallback, so a real press anywhere in the bottom-right corner landed on an
invisible container and the prev/next pair stopped responding entirely — the
`clicked` signal is never emitted, so every signal-level test stayed green.

Two gates hold this: the runtime proof is
`tests/ux_viewer_pointer_flows.rs` (it hit-tests the stage centre and every
chrome button through `gtk_widget_pick` before pressing), and
`tests/ui_viewer_source_structure.rs::overlay_chrome_revealers_are_not_fill_aligned`
reads the revealer's *own* property lines so a plain `cargo test` catches the
regression without a display.

The Revealer sits *outside* the cluster it animates
(`Revealer → Gtk.Box viewer_nav_buttons → prev_btn / next_btn`), which is what
keeps the two visibility features independent. `set_overlay_navigation_visible`
and `set_zoom_controls_visible` reach the cluster through
`prev_btn.parent()` / `zoom_in_btn.parent()`, and that parent is the `Gtk.Box`
both before and after the revealer was added — they toggle the cluster's own
`visible`, while immersive browsing toggles the revealer's `reveal_child`. If a
new container is ever inserted *between* a button and its cluster, those two
helpers will hide only that new layer instead of the cluster, and
`tests/ux_viewer_pointer_flows.rs::header_actions_reach_their_panels_and_mutations`
is what catches it.

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

The preview thumbnail and the original are decoded on two independent async
paths and either can land first. The original is authoritative: once it has
painted for the current `show_at` token (`original_painted_token`), a late
preview-thumbnail callback for that same token must be suppressed
(`original_has_landed`) so it cannot clobber the original. This matters for
non-JPEG sources (PNG screenshots, WebP, HEIC): their thumbnail path decodes
the full-resolution file before downscaling (`generate_via_pixbuf`), so the
thumbnail can finish *after* the lighter original decode and overwrite it —
leaving the viewer permanently stuck on the Medium thumbnail. JPEG thumbnails
take the turbojpeg IDCT fast path and finish first, so only non-JPEG items are
at risk. The token comparison also ensures a *previous* item's original never
suppresses the *current* item's thumbnail after navigation.

Image zoom, portable rotation, and fullscreen-preview controls sit at the image stage's top-right edge so they do not compete with the bottom-right prev/next pair. They read as one cluster of three semantic groups, separated by two `Gtk.Separator`s: zoom (`zoom_out_btn`, `zoom_in_btn`), transform (`rotate_left_btn`, `rotate_right_btn`), view state (`zoom_reset_btn`, `fullscreen_btn`). Declaration order in `viewer-page.blp` *is* visual order — the container is a plain `Gtk.Box`, not a reversed `AdwHeaderBar` — so moving a button means moving it in the template, and `zoom_transform_sep` then `zoom_state_sep` sit at the group boundaries. At identity zoom, show zoom-in, rotate-left, rotate-right, and fullscreen; reveal zoom-out and reset only once the image is enlarged, and hide the rotation buttons while enlarged. A separator follows the group whose buttons disappear: `zoom_transform_sep` is visible only at identity zoom, so the divider never dangles at the cluster end when the transform group is gone, while `zoom_state_sep` always separates zoom from view state. The cluster keeps a single restrained resting outline (`.viewer-zoom-controls` in `base.css`) rather than a per-button capsule, so the buttons stay individually readable as a group and the pair of prev/next arrows at the bottom-right keeps its bare-floating treatment. Zoom and rotation state are viewer-local display transforms, never persisted to the media file, and reset when switching media or opening the editor; videos remain view-only and do not show these image controls.

Viewer previous/next, cancel/close, video playback toggle, image transform, fullscreen-preview, details, edit, and delete shortcuts are routed through the project-wide keyboard subsystem documented in [`keyboard.md`](keyboard.md). Keep visible buttons as the primary affordance and route keyboard actions through the same viewer methods or button signal paths. Do not install touch-only pinch or global swipe controllers on the viewer image stage, because they compete with overlay buttons and keyboard-driven actions. Desktop input is explicit and constrained: Ctrl+wheel on the image stage accumulates 90 degrees per discrete step and zooms only while the image layer is visible and the editor is inactive, while ordinary wheel input proceeds to page scrolling. A primary-button drag pans only when `zoom_scale > 1.0`; its gesture-start pan is stored once and combined with GTK's cumulative drag delta, then clamped to the rotated, contain-fitted image rectangle rather than the full Picture allocation. The editor crop overlay remains the only touch-style direct manipulation outside this constrained viewer input.

## Header Toolbar

The start (left) side of the header carries a date label for the current item,
day precision only (no time), shown for every image and video. It follows the
same date the library sorts and groups by — `MediaItem::sort_datetime`, i.e.
`COALESCE(taken_at, file_mtime)` — so it shows the capture date when present and
falls back to the file mtime (ingestion date) otherwise, and is always populated
once an item is loaded. `update_date_label` runs on every `show_at` and after an
inline rename. The label is revealed by the first `show_at` with no defer:
opening the viewer no longer disables `can_pop`, so AdwHeaderBar's back button
is visible from the start and the date appears alongside it immediately (there
is no initial-open pop guard — an immediate back / Escape / swipe-back after
opening is intentional user input and must work right away). The date is
decoupled from `can_pop`, so opening details or the editor later (which drops
`can_pop` and hides the back button) must not hide the date. The most recent two local days render as the localized
今天 / 昨天; older dates use a locale-appropriate calendar date (zh-CN:
`2026年7月9日`; en: `2026-07-09`). The label carries libadwaita's `title` class
so its font size and weight match the centered file name (it reads as a peer of
the title, not secondary chrome), plus a `viewer-date-label` class for
tabular-nums. It must never change layout or use a hover/glass surface like the
action buttons.

Beside the date sits the position counter `position_label`, formatted as
`{current} / {total}` via `viewer.position.count` (identical string in both
locales; only the digits vary). It is the last child in the header's start
box on purpose: as the rank grows, the label grows rightward into the free
space between start and end groups, so the date and the sync badge never
shift. The counter carries libadwaita's `dim-label` class so it sits behind
the file name and date, plus a `viewer-position-label` class for tabular
figures. It is hidden by default, revealed only once the rank resolves for the
current `show_at` token — an unverified number is never displayed. Each new
rank request also hides the previous item's number up front, and a cancelled
or failed count keeps it hidden, so switching media never leaves the old
「N / M」 on screen.

The rank is `(1-based, total)` from `MediaRepository::position(query, id)`
(`repository.rs`), which delegates to `db::media_position` (`db.rs`). The
underlying SQL runs the same `(filter, sort)` projection the neighbour seek
uses, so the counter cannot disagree with what `←/→` moves to. The viewer
`list_n_items()` (`viewer_page.rs:643`) is **not** a total — it is the length
of the windowed `ListStore`, and using it would be wrong on the first open of
a long grid and on any jump beyond the current window. The query is
asynchronous (`gio::spawn_blocking`) and lands after the frame, so the
navigation critical path is unaffected — the deliberate comment in `db.rs`
calling `media_position` cost two COUNTs is the reason `seek_media_neighbor`
keeps skipping counts. The answer is applied only while both the originating
`show_at` `token` and a per-request `position_request_token` still match, so a
late rank for an item the user has already skipped over cannot repaint the
current frame.

Beside the date sits the position counter `position_label`, formatted as
`{current} / {total}` via `viewer.position.count` (identical string in both
locales; only the digits vary). It is the last child in the header's start
box on purpose: as the rank grows, the label grows rightward into the free
space between start and end groups, so the date and the sync badge never
shift. The counter carries libadwaita's `dim-label` class so it sits behind
the file name and date, plus a `viewer-position-label` class for tabular
figures. It is hidden by default, revealed only once the rank resolves for
the current `show_at` token — an unverified number is never displayed.

The rank is `(1-based, total)` from `MediaRepository::position(query, id)`
(`repository.rs`), which delegates to `db::media_position` (`db.rs`). The
underlying SQL runs the same `(filter, sort)` projection the neighbour seek
uses, so the counter cannot disagree with what `←/→` moves to. The page they
came from (`viewer_page.rs:643 list_n_items`) is **not** a total — it is the
length of the windowed `ListStore`, and using it would be wrong on the first
open of a long grid and on any jump beyond the current window. The query is
asynchronous (`gio::spawn_blocking`) and lands after the frame, so the
navigation critical path is unaffected — the deliberate comment in `db.rs`
calling `media_position` cost two COUNTs is the reason `seek_media_neighbor`
keeps skipping counts. The answer is applied only while both the originating
`show_at` `token` and a per-request `position_request_token` still match, so a
late rank for an item the user has already skipped over cannot repaint the
current frame.

The viewer header carries four actions, left-to-right: favorite, edit, delete,
details. (The earlier add-to-album entry was removed from the
viewer — album assignment for a photo is reached from the photos grid batch
menu instead.) All viewer header buttons share one hover-only treatment: bare
at rest, glass surface on hover/focus, scoped via the `.viewer-chrome` class so
the shared `.glass-toolbar-button` rule used by other pages' headers stays
always-on.

### Immersive Browsing

`F` folds the viewer's own chrome away in place; it does **not** open another
window. The four regions live in `Gtk.Revealer`s so the picture grows into the
freed rows instead of being painted over:

| Region | Revealer | Transition |
|---|---|---|
| `header_bar` | `header_revealer` | `slide_down` |
| `viewer_bottom_stack` (filmstrip) | `filmstrip_revealer` | `slide_up` |
| `viewer_nav_buttons` | `nav_buttons_revealer` | `crossfade` |
| `viewer_zoom_controls` | `zoom_controls_revealer` | `crossfade` |

Rules, all owned by `src/ui/viewer/immersive.rs`:

- A stillness clock has to measure **stillness**, not event delivery. GDK keeps
  sending motion events for a pointer that has not gone anywhere — folding the
  chrome re-targets what sits under it, among other reasons — and treating each
  of those as "the user moved" cancelled and re-armed the pending timer on every
  event, so the timer could never reach its deadline. The chrome then stayed
  pinned open for as long as the pointer rested on the picture, and immersive
  browsing read as a flicker: fold, instant re-reveal, fold, re-reveal.
  `setup_immersive` therefore compares each position against the last
  **accepted** movement and ignores anything within
  `STILLNESS_MOVE_EPS_PX` (1 px). Comparing against the last accepted position
  rather than the previous event is what keeps a slow drag registering: sub-pixel
  steps accumulate until they cross the threshold, while sensor jitter around one
  point never does. The first motion event after a page opens has no baseline and
  is treated as movement, which is the conservative direction — better to show
  chrome than hide it while the user is genuinely moving.
  `src/ui/viewer/immersive/tests.rs::a_pointer_that_did_not_move_is_not_activity`
  pins both halves: a resting pointer must not re-arm the timer, and a real move
  must.
- Pressing `F` folds the chrome right away (`set_immersive(true)` calls
  `set_chrome_revealed(false)` in the same call, which
  `immersive/tests.rs::entering_immersive_folds_chrome_and_leaving_restores_it`
  pins). The 2.5 s stillness clock governs the *re*-fold: movement — anywhere on
  the page, not just over the photo — re-reveals the chrome and re-arms the
  one-shot, so a moving user never actually watches it disappear. A key press
  counts as activity too (`handle_keyboard_action` calls
  `note_immersive_activity` before dispatching), so a keyboard user never has to
  guess whether the buttons are gone.
- The countdown is a single pending source: `arm_immersive_idle` clears the old
  id first, and the callback empties the cell before returning, so
  `clear_immersive_idle` never touches a fired `SourceId` (removing one aborts).
- `Escape` leaves immersion *first* and does not also pop the viewer. Ordering in
  `CancelOrClose`: editor panel → details panel → immersion → pop.
- `immersive_allowed(is_editing, shows_side_panel)` is the gate. Entering is
  refused while editing or while a panel is open, and opening details or starting
  an edit calls `exit_immersive_for_chrome()`. Chrome that vanishes mid-stroke,
  or a panel that folds away behind the user, are the two ways this feature could
  strand someone.
- `set_immersive(false)` always restores all four regions, so no state can leave
  the viewer permanently chromeless. `imp.chrome_revealed` mirrors the template's
  revealed-by-default starting point; without that seed the first fold is a no-op.
- Transition duration is re-read from `motion::enabled()` on every change
  (P1-14). `motion::apply_to()` only runs at construction, so it cannot handle
  the setting being toggled while a page is open — that limitation is documented
  in `ui-liquid-glass.md`, and this path does not depend on it.
- The toast host stays outside all four revealers: a toast that reports a
  successful action must remain visible with the chrome folded, and
  `docs/modules/viewer.md`'s toast-placement contract is untouched.

`Shift+F` is the separate system-fullscreen preview window, described below. The
distinction is the point of the split: the old single `F` opened a *second
top-level window* whose `Escape` returned to a different instance, which is why
the backlog asked for immersion in place.

The image-stage top-right control group also includes a fullscreen preview
action, placed between rotate-right and zoom-in. It opens a separate independent top-level `GtkWindow`,

The favorite button draws `photoviewer-heart-symbolic`, the app's own heart from `data/icons/photoviewer-heart-symbolic.svg`. It used to ask the icon theme for `emblem-favorite-symbolic`, which adwaita-icon-theme does not ship at all (still missing in GNOME 50), so on a stock GNOME system the button drew no icon at all, and the themes that do carry one each drew a different shape. The same mark is now used by the photos page select-all button and the Day-view tile badge; the name lives once in `src/ui/favorite_icon.rs` and is set from Rust rather than written into any `.blp`, so a surface cannot drift back onto a different glyph. Favoriting does not change the button surface — it only recolors the heart icon to a translucent red (`.viewer-favorite-btn.favorite-active` color rule). The button itself never turns red.

The heart's wall is a filled ring, not a `stroke`: GTK recolours a symbolic icon by rewriting the first paint it finds, and a stroked path comes back solid. Its inner contour is wound against the outer one so the hole survives that rewrite whichever way the rewrite handles `fill-rule`. `tests/ui_favorite_icon.rs` renders the mark through GTK under a CSS color and fails if it comes back filled, so "it looks right in a browser" is not the evidence.

**The tint is painted by the app, so it has to be *followed*, not sampled.** GTK will not recolour a raster, so `src/ui/favorite_icon.rs` reads the colour out of the button's style context and hands `favorite_icon::tinted` a texture. The trap is that `gtk_style_context_get_color()` is not a CSS property lookup: it is the colour the context last resolved, and `.glass-toolbar-button` — which both favorite buttons carry — transitions `color` over 120ms. Measured against a mapped window: at the instant `notify::css-classes` fires, the context still answers the colour from *before* the class landed, and a few milliseconds later it answers a blend (at 16ms into the fade, `(255, 230, 227, 252)`). So a single read per class change is never the right colour: the heart was painted from the previous state and stayed there — white while favorited, red while not, the exact inverse of the design.

`favorite_icon::follow` therefore watches everything that can move the colour — a CSS class, a widget state change (`:hover`), being mapped, and a theme switch through one shared `AdwStyleManager` watch — and a frame-clock tick repaints until the colour stops changing, so the heart fades in step with the capsule around it. Two details are load-bearing: the tick must not treat one unchanged frame as settled (the first two frames after a class change report the *same* colour while the fade is still starting, which parked the heart 3% into the transition and read as the wrong colour), and the tint cache must be bounded, because a followed fade contributes a handful of intermediate colours and a cache that only grows turns a session of toggles into tens of megabytes. `assert_icon_follows_the_resolved_colour` in `tests/ui_favorite_icon.rs` presents the page, toggles the class the way `refresh_favorite_button` does, waits out the transition, and requires the painted texture to match the resolved colour in both directions and across a scheme switch. It has to run in a mapped window: unmapped, the context resolves on demand and a one-shot implementation looks correct, which is how this survived a test that added the class and read the icon back in the same breath.

**The wall is 1.4 of the 16 unit box, and it was 0.5, which was invisible.** The mark is a 72 px raster drawn into 16, 18 and 20 px boxes, so wall weight has to be judged after the downscale, not in the source: at 0.5 units the wall is 0.56 px at 18 px, and the artwork then peaked at an alpha of 131/164/180 at 16/18/20 px with not one pixel above 200 — a pale grey smear at every size it is used, while the cloud badge in the same corner, on a full 1 unit stroke, peaked at 238-255. At 1.4 every one of those sizes reaches full opacity (26/36/38 solid pixels). The inner contour is a true inward offset of the outer one by that 1.4 — sides along their own inward normal closed on the miter where they meet the concentric offset lobes (radius 2.0), the two concave corners rounded by a 1.4 arc centred on the outer vertex — because a uniform wall is what makes the weight a number; the previous inner contour drifted between 0.3 and 0.8 units and could not be compared with anything. `assert_wall_is_legible_at_every_drawn_size` in `tests/ui_favorite_icon.rs` renders the mark at all three sizes and fails if the strongest pixel falls below alpha 200, so re-narrowing the wall fails the build rather than shipping a heart nobody can see. The cloud badge's own 1 unit hairline is a separate budget and a separate test (`src/ui/cloud_badge.rs`); keep both legible rather than making them equal.

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
button closes only that preview. Its accelerator is `Shift+F` (`<Shift>F` in the
shortcuts table); the button in the zoom cluster is the pointer path and needs no
key.

## Details And Editor Panels

Details/editor side panels should be treated as overlay chrome, not as layout that changes the main image viewport unexpectedly. Collapsed state should hide or unparent expensive/size-forcing child regions where needed.

The details panel mirrors its row set to the media kind: photos get EXIF camera-parameter rows (aperture, exposure, focal length, location, …), while videos get `ffprobe`-derived rows (duration, codec + profile, frame rate, bit rate, container, device) appended to the same `file_group` via the same dynamic-`ActionRow` mechanism. Video `width`/`height`/`taken_at` light up the shared dimensions/captured rows just like photos. Both sets load asynchronously (`load_camera_details` / `load_video_details`) behind a navigation token so switching items cancels stale loads.

The details panel name row is slightly larger than the other file rows and is activatable. Clicking it opens an inline rename entry that edits the file stem only; the original extension is preserved by the repository rename path, even if the user types a different suffix. Successful renames update the current `ListStore` item, viewer title, details rows, and filmstrip without saving any image/video pixels.

Videos are view-only. Keep the Edit button disabled for `video/*` items and guard the click handler so videos cannot configure `EditorPanel`.

The editor crop selector is drawn as a `GtkDrawingArea` overlay above the viewer `GtkPicture`. It must stay in the image overlay so users can drag the crop rectangle directly over the photo. Coordinate conversion maps the displayed contain-fitted image rectangle back to oriented source-image pixels before updating `EditorPanel`. A hit crop rectangle remains visually selected after click/drag begin so the movable/resizable affordance is obvious.

When the editor sidebar is open, the viewer overlay previous/next navigation
buttons are hidden and the filmstrip stays visible with its thumbnail buttons
insensitive. Keyboard navigation is blocked by the `Editor` keyboard scope.
`navigate_by_delta` also rejects navigation while editing, including filmstrip,
fullscreen-preview and legacy callback entry points; visibility alone is not
an editing lock.

Editor entry uses `displayed_media_id`, the stable identity committed by
`show_at`, rather than the optimistic `current_media_id` that a pending
navigation may already have advanced. Entering editing invalidates the nav
token, restores the displayed identity and render cursor, and clears the
neighbour cache. DB replies, thumbnail-ready replies and timeout fallbacks
cannot switch the photo during editing or revive the cancelled switch after
closing. Closing the editor restores filmstrip interaction and prefetches the
surviving photo's neighbours again. The same thumbnail press must work after
editing ends, and refused navigation must retain pending edits and the save
target.
