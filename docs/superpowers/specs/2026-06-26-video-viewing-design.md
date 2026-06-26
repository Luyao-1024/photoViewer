# Video Viewing Design

## Goal

Add first-class viewing support for local videos while keeping the existing photo browsing, details, albums, trash, and editing flows stable.

## Scope

- Scan both picture roots and video roots.
- Treat pictures and videos as one mixed media library; any supported media file can live under either root.
- Open videos from the existing grid and filmstrip.
- Show video playback in the viewer with a seekable progress bar near the filmstrip.
- Disable editing for videos.

## Architecture

The existing `media_items` table already stores `mime_type`, so video support uses `image/*` and `video/*` as the media discriminator instead of adding a new table. The scanner and watcher move from image-only extension filtering to shared media extension filtering. Viewer display becomes mode-dependent: images use the existing oriented `GtkPicture` path, while videos use `GtkVideo` backed by `GtkMediaFile`.

## UI

The viewer keeps the current chrome model. The stage contains both an image widget and a video widget; only one is visible for the current item. A compact progress slider is shown above the thumbnail strip only for videos. Seeking maps slider percentage to the current `GtkMediaStream` duration.

## Non-Goals

- Video editing.
- Timeline scrubbing thumbnails.
- Dedicated video metadata extraction beyond MIME, size, timestamps, and hash.
- A separate videos page.

## Testing

Focused tests cover root discovery, video MIME extraction, mixed scanning, watcher extension acceptance, and viewer template/edit lock behavior. Verification uses the smallest relevant tests first, then a broader build/test pass when feasible.
