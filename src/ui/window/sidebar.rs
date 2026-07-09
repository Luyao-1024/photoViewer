use super::*;
use gdk_pixbuf::Pixbuf;

use crate::core::thumbnails::ThumbnailSize;
use crate::ui::SquareTile;

// ── Sidebar row builders ──────────────────────────────────────────────────
// Rows share the `.glass-sidebar-row` material (hover/selected glass veil from
// both glass modes); the per-kind classes below only own layout (indentation,
// count badge, section header weight).

/// A plain navigable sidebar row: leading symbolic icon + label.
pub(super) fn build_nav_row(
    label: &str,
    icon_name: &str,
    include_count: bool,
) -> (gtk::ListBoxRow, Option<gtk::Label>) {
    let row = gtk::ListBoxRow::new();
    row.add_css_class("glass-sidebar-row");

    let icon = gtk::Image::from_icon_name(icon_name);
    icon.add_css_class("glass-sidebar-icon");

    let lbl = gtk::Label::builder()
        .label(label)
        .halign(gtk::Align::Start)
        .hexpand(true)
        .css_classes(["glass-sidebar-label"])
        .build();

    let box_ = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(10)
        .build();
    box_.append(&icon);
    box_.append(&lbl);
    let count_label = if include_count {
        let count = gtk::Label::builder()
            .label("0")
            .visible(false)
            .halign(gtk::Align::End)
            .css_classes(["glass-sidebar-count", "photos-sidebar-count"])
            .build();
        box_.append(&count);
        Some(count)
    } else {
        None
    };
    row.set_child(Some(&box_));
    (row, count_label)
}

/// The "Albums" group header: a non-selectable disclosure row. The arrow is
/// returned so the collapse toggle can swap its icon.
pub(super) fn build_albums_header_row(label: &str) -> (gtk::ListBoxRow, gtk::Image) {
    let row = gtk::ListBoxRow::new();
    row.add_css_class("glass-sidebar-row");
    row.add_css_class("glass-sidebar-section");

    let arrow = gtk::Image::from_icon_name("pan-down-symbolic");
    arrow.add_css_class("glass-sidebar-arrow");

    let lbl = gtk::Label::builder()
        .label(label)
        .halign(gtk::Align::Start)
        .hexpand(true)
        .css_classes(["glass-sidebar-section-label"])
        .build();

    let box_ = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(10)
        .build();
    box_.append(&arrow);
    box_.append(&lbl);
    row.set_child(Some(&box_));
    (row, arrow)
}

/// An album sub-row: indented under the Albums header, with the album cover,
/// the album name, and a right-aligned count badge.
pub(super) fn build_album_row(
    album: &Album,
    loader: Option<Arc<ThumbnailLoader>>,
) -> gtk::ListBoxRow {
    let row = gtk::ListBoxRow::new();
    row.add_css_class("glass-sidebar-row");
    row.add_css_class("glass-sidebar-subrow");

    let cover = build_sidebar_album_cover(album, loader);

    let name = gtk::Label::builder()
        .label(album.display_name())
        .halign(gtk::Align::Start)
        .hexpand(true)
        .css_classes(["glass-sidebar-label"])
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .max_width_chars(18)
        .build();

    let count = gtk::Label::builder()
        .label(trf(
            "album.count",
            &[("count", &album.photo_count.to_string())],
        ))
        .halign(gtk::Align::End)
        .css_classes(["glass-sidebar-count"])
        .build();

    let box_ = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(10)
        .build();
    box_.append(&cover);
    box_.append(&name);
    box_.append(&count);
    row.set_child(Some(&box_));
    row
}

pub(super) fn update_album_row_in_place(
    row: &gtk::ListBoxRow,
    previous: &Album,
    album: &Album,
    loader: Option<Arc<ThumbnailLoader>>,
) {
    if let Some(name) = find_label_with_css_class(row.upcast_ref(), "glass-sidebar-label") {
        name.set_label(&album.display_name());
    }
    if let Some(count) = find_label_with_css_class(row.upcast_ref(), "glass-sidebar-count") {
        count.set_label(&trf(
            "album.count",
            &[("count", &album.photo_count.to_string())],
        ));
    }
    if previous.cover_uri != album.cover_uri || previous.last_modified != album.last_modified {
        replace_sidebar_album_cover(row, album, loader);
    }
}

fn find_label_with_css_class(widget: &gtk::Widget, class_name: &str) -> Option<gtk::Label> {
    if widget.has_css_class(class_name) {
        if let Ok(label) = widget.clone().downcast::<gtk::Label>() {
            return Some(label);
        }
    }
    let mut child = widget.first_child();
    while let Some(current) = child {
        if let Some(found) = find_label_with_css_class(&current, class_name) {
            return Some(found);
        }
        child = current.next_sibling();
    }
    None
}

pub(super) fn same_sidebar_album_identities(current: &[Album], next: &[Album]) -> bool {
    current.len() == next.len()
        && current
            .iter()
            .zip(next)
            .all(|(current, next)| same_sidebar_album_identity(current, next))
}

pub(super) fn same_sidebar_album_identity(current: &Album, next: &Album) -> bool {
    current.folder_path == next.folder_path && current.is_virtual == next.is_virtual
}

pub(super) fn sidebar_album_identities_are_ordered_subset(
    current: &[Album],
    next: &[Album],
) -> bool {
    if next.len() > current.len() {
        return false;
    }
    let mut search_from = 0;
    for next_album in next {
        let Some(offset) = current[search_from..]
            .iter()
            .position(|current_album| same_sidebar_album_identity(current_album, next_album))
        else {
            return false;
        };
        search_from += offset + 1;
    }
    true
}

pub(super) fn find_sidebar_album_identity_index(albums: &[Album], needle: &Album) -> Option<usize> {
    albums
        .iter()
        .position(|album| same_sidebar_album_identity(album, needle))
}

pub(super) fn sidebar_list_child_count(list: &gtk::ListBox) -> u32 {
    list.observe_children().n_items()
}

pub(super) fn sidebar_album_summary(albums: &[Album]) -> String {
    const MAX_ITEMS: usize = 8;
    let mut parts = albums
        .iter()
        .take(MAX_ITEMS)
        .map(sidebar_album_identity_for_log)
        .collect::<Vec<_>>();
    if albums.len() > MAX_ITEMS {
        parts.push(format!("...(+{})", albums.len() - MAX_ITEMS));
    }
    parts.join(" | ")
}

pub(super) fn sidebar_album_identity_for_log(album: &Album) -> String {
    let kind = if album.is_virtual {
        "virtual"
    } else {
        "folder"
    };
    format!(
        "{}:{} count={} name={}",
        kind,
        album.folder_path.display(),
        album.photo_count,
        album.display_name()
    )
}

pub(super) fn replace_sidebar_album_cover(
    row: &gtk::ListBoxRow,
    album: &Album,
    loader: Option<Arc<ThumbnailLoader>>,
) {
    let Some(cover) = find_sidebar_album_cover(row.upcast_ref()) else {
        return;
    };
    let Some(parent) = cover.parent().and_downcast::<gtk::Box>() else {
        return;
    };
    let replacement = build_sidebar_album_cover(album, loader);
    parent.remove(&cover);
    parent.prepend(&replacement);
}

pub(super) fn find_sidebar_album_cover(widget: &gtk::Widget) -> Option<SquareTile> {
    if widget.has_css_class("glass-sidebar-cover") {
        if let Ok(tile) = widget.clone().downcast::<SquareTile>() {
            return Some(tile);
        }
    }
    let mut child = widget.first_child();
    while let Some(current) = child {
        if let Some(found) = find_sidebar_album_cover(&current) {
            return Some(found);
        }
        child = current.next_sibling();
    }
    None
}

pub(super) fn build_sidebar_album_cover(
    album: &Album,
    loader: Option<Arc<ThumbnailLoader>>,
) -> SquareTile {
    let tile = SquareTile::new();
    tile.set_target(24);
    tile.set_halign(gtk::Align::Center);
    tile.set_valign(gtk::Align::Center);
    tile.set_hexpand(false);
    tile.set_vexpand(false);
    tile.add_css_class("glass-sidebar-cover");
    tile.set_paintable(Some(&sidebar_cover_placeholder_texture()));

    let Some(loader) = loader else {
        return tile;
    };
    let Some(cover_uri) = album.cover_uri.as_ref().cloned() else {
        return tile;
    };

    let (tx, rx) = tokio::sync::oneshot::channel();
    loader.request(
        cover_uri,
        ThumbnailSize::Small,
        Some(std::time::SystemTime::from(album.last_modified)),
        tx,
        crate::core::thumbnails::TIER_NORMAL,
    );
    let tile_weak = tile.downgrade();
    gtk::glib::spawn_future_local(async move {
        let Ok(loaded) = rx.await else {
            return;
        };
        if let Some(tile) = tile_weak.upgrade() {
            tile.set_paintable(Some(&loaded.texture));
        }
    });

    tile
}

pub(super) fn sidebar_cover_placeholder_texture() -> gtk::gdk::Texture {
    let pixbuf = Pixbuf::new(gdk_pixbuf::Colorspace::Rgb, true, 8, 2, 2)
        .expect("allocate 2x2 sidebar album cover placeholder pixbuf");
    pixbuf.fill(0xC8C8C8FF);
    gtk::gdk::Texture::for_pixbuf(&pixbuf)
}

pub(crate) fn refresh_albums_sidebar(nav: &adw::NavigationView) {
    if let Some(window) = nav
        .ancestor(MainWindow::static_type())
        .and_downcast::<MainWindow>()
    {
        window.refresh_sidebar_snapshot_async();
    }
}

#[tracing::instrument(name = "sidebar:load_album_snapshot", skip(pool))]
pub(super) fn load_sidebar_album_snapshot(pool: &DbPool) -> SidebarAlbumSnapshot {
    let live_count = crate::core::repository::MediaRepository::new(pool.clone())
        .count(crate::core::repository::MediaQuery::LiveAll)
        .ok();
    SidebarAlbumSnapshot {
        albums: list_with_favorites(pool).unwrap_or_default(),
        media_type_albums: list_media_type_albums(pool).unwrap_or_default(),
        live_count,
    }
}
