# Viewer Details Panel Design

## Goal

Viewer opens with the current image name visible without extra interaction, and provides a right-side sliding details panel for file and shooting metadata.

## User Experience

- The viewer page title is the current image display name.
- The header bar adds an information button on the right.
- Clicking the information button reveals a right-aligned details panel over the image area; clicking it again or the panel close button hides the panel.
- Keyboard navigation updates the image name and, when visible, the details panel contents.

## Details Content

The panel shows:

- File: name, full path, folder, MIME type, dimensions, file size, modified time.
- Dates: captured time from indexed metadata when available.
- EXIF: every readable EXIF field from the image file, with common camera/shooting rows appearing under the same list.

Missing optional values render as `Not available` so the panel remains stable for images without EXIF.

## Architecture

- Extend `core::metadata::RawMetadata` with `exif_fields: Vec<ExifField>`.
- Keep the DB schema unchanged; persisted list views continue using the existing basic metadata.
- `ViewerPage` uses the current `MediaItem` for indexed/basic values and calls `metadata::extract` on demand to populate raw EXIF rows.
- The Blueprint template owns the panel layout; Rust only toggles visibility and fills row text.

## Testing

- Metadata tests cover EXIF field extraction and the no-EXIF empty case.
- Viewer tests cover display name helper behavior without requiring a visible GTK session.
