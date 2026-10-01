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
//!
//! Every placeholder that describes a *problem* must offer a way out: use
//! [`add_action`] to attach one button below the text. A status page with no
//! action leaves the user with nothing to do but restart the app.
use crate::core::i18n::{tr, trf};
use gtk4 as gtk;
use gtk4::prelude::*;
use libadwaita as adw;
use std::rc::Rc;

/// Attach a single action button under a status page's text and return it, so
/// callers can keep a handle for sensitivity or label updates. `label` is
/// already translated.
pub fn add_action(page: &adw::StatusPage, label: &str, on_activate: Rc<dyn Fn()>) -> gtk::Button {
    let button = gtk::Button::builder()
        .label(label)
        .css_classes(["pill", "suggested-action"])
        .build();
    button.connect_clicked(move |_| on_activate());
    page.set_child(Some(&button));
    button
}

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

/// Description text for a failed scan. A page built once and reused can update
/// just this string, which is why it is split out of [`scan_error`].
pub fn scan_error_text(message: Option<&str>) -> String {
    match message.map(str::trim).filter(|text| !text.is_empty()) {
        Some(reason) => trf(
            "empty.scan_failed.description_with_reason",
            &[("reason", reason)],
        ),
        None => tr("empty.scan_failed.description"),
    }
}

/// Error state for scan failures. `msg` is shown as the description so
/// the user sees the actual failure reason (path, permission, etc.). An empty
/// `msg` falls back to the generic text — a warning icon with no description
/// reads as a rendering bug, not as an error.
pub fn scan_error(msg: &str) -> adw::StatusPage {
    let p = adw::StatusPage::builder()
        .icon_name("dialog-warning-symbolic")
        .title(tr("empty.scan_failed.title"))
        .description(scan_error_text(Some(msg)))
        .build();
    p.add_css_class("compact");
    p
}

/// Loading state — used during initial scan / refresh while data is
/// being fetched from disk and indexed in the database.
pub fn loading() -> adw::StatusPage {
    adw::StatusPage::builder()
        .title(tr("empty.loading"))
        .build()
}

/// Indexing state for the startup scan. Distinct from [`loading`] because the
/// pass can run for a long time and has to say so: an empty grid during a scan
/// is not an empty library. The spinner is the page's child, so callers must
/// not attach an action button here.
pub fn scanning() -> adw::StatusPage {
    let page = adw::StatusPage::builder()
        .title(tr("empty.scanning.title"))
        .description(tr("empty.scanning.description"))
        .build();
    page.add_css_class("compact");
    let spinner = gtk::Spinner::new();
    spinner.set_size_request(32, 32);
    page.set_child(Some(&spinner));
    page
}
