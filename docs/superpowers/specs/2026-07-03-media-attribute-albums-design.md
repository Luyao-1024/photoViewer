# Media Attribute Albums Design

## Goal

Add optional media attribute categories under the sidebar "Media Types" group. Empty categories are hidden, and the whole "Media Types" group is hidden when no attribute category has any media.

## Scope

The first attribute categories are:

- Motion photos: existing `media_subkind = 'motion_photo'`.
- Animated images: persisted in `media_attributes` as `animated: true`.
- HDR media: persisted in `media_attributes` as `hdr: true`.

These categories are virtual albums. They reuse the existing AlbumDetailPage flow and repository paging/navigation so large libraries still query the database instead of filtering the bounded GTK model.

## Data Model

No new table is required. `media_items.media_attributes` remains JSON and gains top-level boolean flags for general media attributes. These flags can coexist with the existing `motion_photo` object, so a single file may appear in more than one media type album.

## UI Behavior

`list_media_type_albums` returns only non-empty virtual albums. Sidebar media type rows are rebuilt from that list. If the list is empty, both the media type header and media type scroll region are hidden.

## Detection

GIF files are supported and marked animated by extension/MIME. HDR is intentionally conservative in this change: media is classified as HDR only when persisted metadata already carries the `hdr` flag. This avoids false positives until reliable HDR parsing is added for HEIC/video metadata.

## Verification

Focused tests cover album visibility, attribute filtering, repository queries, and the sidebar hiding rule.
