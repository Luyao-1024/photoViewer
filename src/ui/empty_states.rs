//! Empty-state `AdwStatusPage` factories.
//!
//! Each view (Photos, Albums, Trash, AlbumDetail) shows a friendly
//! `AdwStatusPage` when its underlying data list is empty. The factories
//! are pure (no I/O) — pages decide *whether* to show an empty state
//! based on data shape, and call into this module to obtain the widget.
//!
//! All factories return owned `adw::StatusPage` widgets ready to be
//! inserted into a container (typically swapped in place of the normal
//! grid/flow-box). `add_css_class("compact")` is applied to `no_photos`
//! to keep the title+icon size proportional on small pages.
use crate::core::i18n::tr;
use gtk4::prelude::*;
use libadwaita as adw;

/// Empty state for the main Photos view: no photos have been imported yet.
pub fn no_photos() -> adw::StatusPage {
    let p = adw::StatusPage::builder()
        .icon_name("image-x-generic-symbolic")
        .title(tr("empty.no_photos.title"))
        .description(tr("empty.no_photos.description"))
        .build();
    p.add_css_class("compact");
    p
}

/// Empty state for the Albums view: no folder-as-album has been discovered.
pub fn no_albums() -> adw::StatusPage {
    adw::StatusPage::builder()
        .icon_name("folder-symbolic")
        .title(tr("empty.no_albums.title"))
        .description(tr("empty.no_albums.description"))
        .build()
}

/// Empty state for the Trash view: no deleted photos in the trash.
pub fn empty_trash() -> adw::StatusPage {
    adw::StatusPage::builder()
        .icon_name("user-trash-symbolic")
        .title(tr("empty.trash_empty.title"))
        .description(tr("empty.trash_empty.description"))
        .build()
}

/// Empty state for a single album page: the album contains no photos
/// (folder exists but holds nothing matching the media filter).
pub fn no_album_photos() -> adw::StatusPage {
    adw::StatusPage::builder()
        .icon_name("image-missing-symbolic")
        .title(tr("empty.no_album_photos.title"))
        .description(tr("empty.no_album_photos.description"))
        .build()
}

/// Error state for scan failures. `msg` is shown as the description so
/// the user sees the actual failure reason (path, permission, etc.).
pub fn scan_error(msg: &str) -> adw::StatusPage {
    adw::StatusPage::builder()
        .icon_name("dialog-warning-symbolic")
        .title(tr("empty.scan_failed.title"))
        .description(msg)
        .build()
}

/// Loading state — used during initial scan / refresh while data is
/// being fetched from disk and indexed in the database.
pub fn loading() -> adw::StatusPage {
    adw::StatusPage::builder()
        .title(tr("empty.loading"))
        .build()
}
