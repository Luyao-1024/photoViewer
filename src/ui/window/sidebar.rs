use super::{MainWindow, SidebarAlbumSnapshot, SIDEBAR_SNAPSHOT_TRACE_ID};
use gdk_pixbuf::Pixbuf;

use crate::core::albums::{list_media_type_albums, list_with_favorites, Album};
use crate::core::db::DbPool;
use crate::core::i18n::trf;
use crate::core::thumbnails::{ThumbnailLoader, ThumbnailSize};
use crate::ui::SquareTile;
use gtk4 as gtk;
use gtk4::subclass::prelude::ObjectSubclassIsExt;
use gtk4::{glib, prelude::*};
use libadwaita as adw;
use std::sync::atomic::Ordering;
use std::sync::Arc;

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

/// Build the visual contents of one virtual album item.  This deliberately
/// keeps the same material classes and child layout as `build_album_row`; the
/// outer selection row is now owned by `GtkListView` instead of `GtkListBox`.
fn build_virtual_album_item(album: &Album, loader: Option<Arc<ThumbnailLoader>>) -> gtk::Box {
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
    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(10)
        .css_classes(["glass-sidebar-row", "glass-sidebar-subrow"])
        .build();
    content.append(&cover);
    content.append(&name);
    content.append(&count);
    content
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

impl MainWindow {
    pub(super) fn ensure_virtual_album_list(&self) {
        if self.imp().album_model.borrow().is_some() {
            return;
        }
        let model = gtk::gio::ListStore::new::<glib::BoxedAnyObject>();
        let selection = gtk::MultiSelection::new(Some(model.clone()));
        let factory = gtk::SignalListItemFactory::new();
        let weak = self.downgrade();
        factory.connect_bind(move |_, object| {
            let Ok(list_item) = object.clone().downcast::<gtk::ListItem>() else {
                return;
            };
            let Some(window) = weak.upgrade() else {
                return;
            };
            let Some(album) = list_item
                .item()
                .and_then(|item| item.downcast::<glib::BoxedAnyObject>().ok())
                .map(|boxed| boxed.borrow::<Album>().clone())
            else {
                return;
            };
            let content =
                build_virtual_album_item(&album, window.imp().loader.borrow().as_ref().cloned());
            window.attach_album_dnd(
                content.upcast_ref(),
                album.folder_path.to_string_lossy().into_owned(),
            );
            window.attach_album_context_menu(content.upcast_ref(), album);
            list_item.set_child(Some(&content));
        });
        factory.connect_unbind(|_, object| {
            if let Ok(list_item) = object.clone().downcast::<gtk::ListItem>() {
                list_item.set_child(Option::<&gtk::Widget>::None);
            }
        });
        self.imp().album_list.set_factory(Some(&factory));
        self.imp().album_list.set_model(Some(&selection));
        *self.imp().album_model.borrow_mut() = Some(model);
        *self.imp().album_selection.borrow_mut() = Some(selection);
    }

    pub(super) fn clear_album_selection(&self) {
        if let Some(selection) = self.imp().album_selection.borrow().as_ref() {
            selection.unselect_all();
        }
    }

    pub(super) fn album_item_is_selected(&self, position: u32) -> bool {
        self.imp()
            .album_selection
            .borrow()
            .as_ref()
            .is_some_and(|selection| selection.is_selected(position))
    }

    pub(super) fn unselect_album_position(&self, position: u32) {
        if let Some(selection) = self.imp().album_selection.borrow().as_ref() {
            selection.unselect_item(position);
        }
    }

    pub(super) fn select_album_position(&self, position: u32) {
        if let Some(selection) = self.imp().album_selection.borrow().as_ref() {
            self.imp().selecting_programmatically.set(true);
            selection.select_item(position, true);
            self.imp().selecting_programmatically.set(false);
        }
    }

    pub(super) fn select_album_by_identity(&self, needle: &Album) {
        if let Some(position) =
            find_sidebar_album_identity_index(&self.imp().album_targets.borrow(), needle)
        {
            self.select_album_position(position as u32);
        }
    }

    /// Insert the folder + virtual albums in the dedicated album list, fetched
    /// from the current DB snapshot. Called once after `set_resources` (and
    /// again by [`Self::refresh_album_rows`] on live changes). Safe to call
    /// before `connect_sidebar`; it only touches album rows + album targets.
    pub fn populate_album_rows(&self) {
        self.ensure_virtual_album_list();
        self.update_photos_count_label_from_db();
        self.rebuild_album_rows();
        self.rebuild_media_type_rows();
    }

    #[tracing::instrument(name = "sidebar:rebuild_album_rows", skip(self))]
    pub(super) fn rebuild_album_rows(&self) {
        let Some(pool) = self.imp().pool.borrow().clone() else {
            return;
        };
        let albums = list_with_favorites(&pool).unwrap_or_default();
        self.apply_album_rows(albums);
    }

    #[tracing::instrument(name = "sidebar:apply_album_rows", skip(self, albums))]
    fn apply_album_rows(&self, albums: Vec<Album>) {
        self.ensure_virtual_album_list();
        let current_targets = self.imp().album_targets.borrow().clone();
        let same_identities = same_sidebar_album_identities(&current_targets, &albums);
        tracing::debug!(
            target: crate::core::log_targets::BROWSING,
            "SIDEBAR_TRACE album_rows_begin current_targets={} model_items={} incoming={} same_identities={} virtualized=true expanded={} scroll_visible={} scroll_height={} wrapper_height={} current=[{}] incoming=[{}]",
            current_targets.len(),
            self.imp().album_model.borrow().as_ref().map_or(0, |model| model.n_items()),
            albums.len(),
            same_identities,
            self.imp().albums_expanded.get(),
            self.imp().album_scroll.is_visible(),
            self.imp().album_scroll.height(),
            self.imp().album_trash_wrapper.height(),
            sidebar_album_summary(&current_targets),
            sidebar_album_summary(&albums)
        );
        let album_count = albums.len();
        let expanded = self.imp().albums_expanded.get();
        self.imp().album_scroll.set_visible(expanded);
        let items = albums
            .iter()
            .cloned()
            .map(glib::BoxedAnyObject::new)
            .collect::<Vec<_>>();
        self.imp().selecting_programmatically.set(true);
        if let Some(model) = self.imp().album_model.borrow().as_ref() {
            model.splice(0, model.n_items(), &items);
        }
        *self.imp().album_targets.borrow_mut() = albums;
        self.imp().selecting_programmatically.set(false);

        self.reselect_active_album_row();
        tracing::debug!(
            target: crate::core::log_targets::BROWSING,
            "SIDEBAR_ALBUM_MODEL_REPLACED rows={} realized_rows=viewport_only",
            album_count
        );
        self.log_sidebar_layout_state("album_rows_rebuild_after");
        self.log_sidebar_layout_state_next_idle("album_rows_rebuild_after");
    }

    fn rebuild_media_type_rows(&self) {
        let Some(pool) = self.imp().pool.borrow().clone() else {
            return;
        };
        let albums = list_media_type_albums(&pool).unwrap_or_default();
        self.apply_media_type_rows(albums);
    }

    fn apply_media_type_rows(&self, albums: Vec<Album>) {
        let media_type_list = self.imp().media_type_list.get();

        let has_media_types = !albums.is_empty();
        let expanded = self.imp().media_types_expanded.get();
        let header_visible_before = self.imp().media_type_header_list.is_visible();
        let scroll_visible_before = self.imp().media_type_scroll.is_visible();
        self.imp()
            .media_type_header_list
            .set_visible(has_media_types);
        self.imp()
            .media_type_scroll
            .set_visible(has_media_types && expanded);
        tracing::debug!(
            target: crate::core::log_targets::BROWSING,
            "SIDEBAR_TRACE media_type_visibility has_media_types={} expanded={} header_visible_before={} header_visible_after={} scroll_visible_before={} scroll_visible_after={} scroll_height={}",
            has_media_types,
            expanded,
            header_visible_before,
            self.imp().media_type_header_list.is_visible(),
            scroll_visible_before,
            self.imp().media_type_scroll.is_visible(),
            self.imp().media_type_scroll.height()
        );

        let current_targets = self.imp().media_type_targets.borrow().clone();
        let current_rows = self.imp().media_type_rows.borrow().clone();
        let same_identities = same_sidebar_album_identities(&current_targets, &albums);
        let ordered_subset = current_rows.len() == current_targets.len()
            && sidebar_album_identities_are_ordered_subset(&current_targets, &albums);
        tracing::debug!(
            target: crate::core::log_targets::BROWSING,
            "SIDEBAR_TRACE media_type_rows_begin current_targets={} current_rows={} list_children={} incoming={} same_identities={} ordered_subset={} current=[{}] incoming=[{}]",
            current_targets.len(),
            current_rows.len(),
            sidebar_list_child_count(&media_type_list),
            albums.len(),
            same_identities,
            ordered_subset,
            sidebar_album_summary(&current_targets),
            sidebar_album_summary(&albums)
        );
        if same_identities && current_rows.len() == albums.len() {
            let loader = self.imp().loader.borrow().as_ref().cloned();
            for ((row, previous), album) in current_rows
                .iter()
                .zip(current_targets.iter())
                .zip(albums.iter())
            {
                update_album_row_in_place(row, previous, album, loader.clone());
            }
            *self.imp().media_type_targets.borrow_mut() = albums;
            self.reselect_active_album_row();
            self.log_sidebar_layout_state("media_type_rows_same_identities_after");
            self.log_sidebar_layout_state_next_idle("media_type_rows_same_identities_after");
            return;
        }
        if ordered_subset {
            let loader = self.imp().loader.borrow().as_ref().cloned();
            let mut next_rows = Vec::with_capacity(albums.len());
            let mut previous_targets = Vec::with_capacity(albums.len());
            for album in &albums {
                if let Some(index) = find_sidebar_album_identity_index(&current_targets, album) {
                    next_rows.push(current_rows[index].clone());
                    previous_targets.push(current_targets[index].clone());
                }
            }
            for ((row, previous), album) in next_rows
                .iter()
                .zip(previous_targets.iter())
                .zip(albums.iter())
            {
                update_album_row_in_place(row, previous, album, loader.clone());
            }
            for (index, row) in current_rows.iter().enumerate().rev() {
                if !albums
                    .iter()
                    .any(|album| same_sidebar_album_identity(&current_targets[index], album))
                {
                    tracing::debug!(
                        target: crate::core::log_targets::BROWSING,
                        "SIDEBAR_TRACE media_type_rows_remove_missing index={} identity={}",
                        index,
                        sidebar_album_identity_for_log(&current_targets[index])
                    );
                    media_type_list.remove(row);
                }
            }
            *self.imp().media_type_rows.borrow_mut() = next_rows;
            *self.imp().media_type_targets.borrow_mut() = albums;
            self.reselect_active_album_row();
            self.log_sidebar_layout_state("media_type_rows_ordered_subset_after");
            self.log_sidebar_layout_state_next_idle("media_type_rows_ordered_subset_after");
            return;
        }
        tracing::debug!(
            target: crate::core::log_targets::BROWSING,
            "SIDEBAR_TRACE media_type_rows_rebuild_clear begin list_children={} current_rows={} incoming={} scroll_visible={} scroll_height={} reason=identity_insert_or_reorder current=[{}] incoming=[{}]",
            sidebar_list_child_count(&media_type_list),
            current_rows.len(),
            albums.len(),
            self.imp().media_type_scroll.is_visible(),
            self.imp().media_type_scroll.height(),
            sidebar_album_summary(&current_targets),
            sidebar_album_summary(&albums)
        );
        while let Some(child) = media_type_list.first_child() {
            media_type_list.remove(&child);
        }
        self.imp().media_type_rows.borrow_mut().clear();
        self.imp().media_type_targets.borrow_mut().clear();

        for album in albums {
            let row = build_album_row(&album, self.imp().loader.borrow().as_ref().cloned());
            row.set_visible(true);
            media_type_list.append(&row);
            self.imp().media_type_rows.borrow_mut().push(row);
            self.imp().media_type_targets.borrow_mut().push(album);
        }

        self.reselect_active_album_row();
        self.log_sidebar_layout_state("media_type_rows_rebuild_after");
        self.log_sidebar_layout_state_next_idle("media_type_rows_rebuild_after");
    }

    #[tracing::instrument(name = "sidebar:apply_album_snapshot", skip(self, snapshot))]
    pub(super) fn apply_sidebar_album_snapshot(&self, snapshot: SidebarAlbumSnapshot) {
        tracing::debug!(
            target: crate::core::log_targets::BROWSING,
            "SIDEBAR_TRACE snapshot_apply_begin live_count={:?} albums={} media_types={} albums_summary=[{}] media_types_summary=[{}]",
            snapshot.live_count,
            snapshot.albums.len(),
            snapshot.media_type_albums.len(),
            sidebar_album_summary(&snapshot.albums),
            sidebar_album_summary(&snapshot.media_type_albums)
        );
        self.log_sidebar_layout_state("snapshot_apply_begin");
        if let Some(live_count) = snapshot.live_count {
            self.set_photos_count_label(live_count);
        }
        self.apply_album_rows(snapshot.albums);
        self.apply_media_type_rows(snapshot.media_type_albums);
        self.log_sidebar_layout_state("snapshot_apply_end");
        self.log_sidebar_layout_state_next_idle("snapshot_apply_end");
    }

    pub(super) fn install_sidebar_layout_trace(&self) {
        if self.imp().sidebar_layout_trace_installed.replace(true) {
            return;
        }
        self.connect_sidebar_widget_trace(
            "album_trash_wrapper",
            self.imp().album_trash_wrapper.upcast_ref(),
        );
        self.connect_sidebar_widget_trace("album_scroll", self.imp().album_scroll.upcast_ref());
        self.connect_sidebar_widget_trace("album_list", self.imp().album_list.upcast_ref());
        self.connect_sidebar_widget_trace(
            "media_type_header_list",
            self.imp().media_type_header_list.upcast_ref(),
        );
        self.connect_sidebar_widget_trace(
            "media_type_scroll",
            self.imp().media_type_scroll.upcast_ref(),
        );
        self.connect_sidebar_widget_trace(
            "media_type_list",
            self.imp().media_type_list.upcast_ref(),
        );
        self.connect_sidebar_widget_trace("trash_list", self.imp().trash_list.upcast_ref());
        self.connect_sidebar_widget_trace("sidebar_spacer", self.imp().sidebar_spacer.upcast_ref());
        tracing::debug!(
            target: crate::core::log_targets::BROWSING,
            "SIDEBAR_TRACE layout_trace_installed"
        );
        self.log_sidebar_layout_state("layout_trace_installed");
    }

    fn connect_sidebar_widget_trace(&self, name: &'static str, widget: &gtk::Widget) {
        widget.connect_notify_local(
            Some("height"),
            glib::clone!(@weak self as window => move |widget, _| {
                tracing::debug!(
                    target: crate::core::log_targets::BROWSING,
                    "SIDEBAR_TRACE widget_height name={} visible={} mapped={} width={} height={}",
                    name,
                    widget.is_visible(),
                    widget.is_mapped(),
                    widget.width(),
                    widget.height()
                );
                window.log_sidebar_layout_state(&format!("height_notify:{name}"));
            }),
        );
        widget.connect_notify_local(
            Some("visible"),
            glib::clone!(@weak self as window => move |widget, _| {
                tracing::debug!(
                    target: crate::core::log_targets::BROWSING,
                    "SIDEBAR_TRACE widget_visible name={} visible={} mapped={} width={} height={}",
                    name,
                    widget.is_visible(),
                    widget.is_mapped(),
                    widget.width(),
                    widget.height()
                );
                window.log_sidebar_layout_state(&format!("visible_notify:{name}"));
                window.log_sidebar_layout_state_next_idle(format!("visible_notify:{name}"));
            }),
        );
    }

    pub(super) fn log_sidebar_layout_state_next_idle(&self, stage: impl Into<String>) {
        let stage = stage.into();
        let weak = self.downgrade();
        glib::idle_add_local_once(move || {
            if let Some(window) = weak.upgrade() {
                window.log_sidebar_layout_state(&format!("{stage}:idle"));
            }
        });
    }

    pub(super) fn log_sidebar_layout_state(&self, stage: &str) {
        let active_album = self
            .imp()
            .active_album
            .borrow()
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "none".into());
        tracing::debug!(
            target: crate::core::log_targets::BROWSING,
            "SIDEBAR_TRACE layout stage={} albums_expanded={} media_types_expanded={} album_scroll_visible={} album_scroll_mapped={} album_scroll_wh={}x{} album_list_children={} album_list_wh={}x{} album_targets={} media_type_header_visible={} media_type_scroll_visible={} media_type_scroll_mapped={} media_type_scroll_wh={}x{} media_type_list_children={} media_type_targets={} wrapper_vexpand={} wrapper_wh={}x{} spacer_vexpand={} spacer_visible={} spacer_wh={}x{} trash_children={} active_album={}",
            stage,
            self.imp().albums_expanded.get(),
            self.imp().media_types_expanded.get(),
            self.imp().album_scroll.is_visible(),
            self.imp().album_scroll.is_mapped(),
            self.imp().album_scroll.width(),
            self.imp().album_scroll.height(),
            self.imp().album_model.borrow().as_ref().map_or(0, |model| model.n_items()),
            self.imp().album_list.width(),
            self.imp().album_list.height(),
            self.imp().album_targets.borrow().len(),
            self.imp().media_type_header_list.is_visible(),
            self.imp().media_type_scroll.is_visible(),
            self.imp().media_type_scroll.is_mapped(),
            self.imp().media_type_scroll.width(),
            self.imp().media_type_scroll.height(),
            sidebar_list_child_count(&self.imp().media_type_list),
            self.imp().media_type_targets.borrow().len(),
            self.imp().album_trash_wrapper.vexpands(),
            self.imp().album_trash_wrapper.width(),
            self.imp().album_trash_wrapper.height(),
            self.imp().sidebar_spacer.vexpands(),
            self.imp().sidebar_spacer.is_visible(),
            self.imp().sidebar_spacer.width(),
            self.imp().sidebar_spacer.height(),
            sidebar_list_child_count(&self.imp().trash_list),
            active_album
        );
    }

    fn reselect_active_album_row(&self) {
        let active = match self.imp().active_album.borrow().clone() {
            Some(path) => path,
            None => return,
        };
        let idx = self
            .imp()
            .album_targets
            .borrow()
            .iter()
            .position(|album| album.folder_path == active);
        if let Some(i) = idx {
            self.select_album_position(i as u32);
        }
        let media_type_list = self.imp().media_type_list.get();
        let idx = self
            .imp()
            .media_type_targets
            .borrow()
            .iter()
            .position(|album| album.folder_path == active);
        if let Some(i) = idx {
            if let Some(row) = media_type_list.row_at_index(i as i32) {
                self.imp().selecting_programmatically.set(true);
                media_type_list.select_row(Some(&row));
                self.imp().selecting_programmatically.set(false);
            }
        }
    }

    /// Collapse/expand the Albums group: swap the disclosure arrow and toggle
    /// the dedicated album scroll region. Rows remain mounted in `album_list`
    /// so their drag order and album target indices stay stable.
    /// When expanded, the album-trash wrapper fills available space; the
    /// scrolled window sizes to content (no vexpand). When collapsed, the
    /// spacer expands so Settings stays pinned to the bottom.
    pub fn toggle_albums_expanded(&self) {
        tracing::debug!(
            target: crate::core::log_targets::BROWSING,
            "SIDEBAR_TRACE toggle_albums_expanded begin current_expanded={}",
            self.imp().albums_expanded.get()
        );
        self.log_sidebar_layout_state("toggle_albums_expanded_before");
        let expanded = !self.imp().albums_expanded.get();
        self.imp().albums_expanded.set(expanded);
        if let Some(arrow) = self.imp().albums_arrow.borrow().clone() {
            if expanded {
                arrow.remove_css_class("collapsed");
            } else {
                arrow.add_css_class("collapsed");
            }
        }
        self.imp().album_scroll.set_visible(expanded);
        self.imp().album_trash_wrapper.set_vexpand(expanded);
        self.imp().sidebar_spacer.set_vexpand(!expanded);
        tracing::debug!(
            target: crate::core::log_targets::BROWSING,
            "SIDEBAR_TRACE toggle_albums_expanded end expanded={}",
            expanded
        );
        self.log_sidebar_layout_state("toggle_albums_expanded_after");
        self.log_sidebar_layout_state_next_idle("toggle_albums_expanded_after");
    }

    pub fn toggle_media_types_expanded(&self) {
        tracing::debug!(
            target: crate::core::log_targets::BROWSING,
            "SIDEBAR_TRACE toggle_media_types_expanded begin current_expanded={}",
            self.imp().media_types_expanded.get()
        );
        self.log_sidebar_layout_state("toggle_media_types_expanded_before");
        let expanded = !self.imp().media_types_expanded.get();
        self.imp().media_types_expanded.set(expanded);
        if let Some(arrow) = self.imp().media_types_arrow.borrow().clone() {
            if expanded {
                arrow.remove_css_class("collapsed");
            } else {
                arrow.add_css_class("collapsed");
            }
        }
        self.imp().media_type_scroll.set_visible(expanded);
        tracing::debug!(
            target: crate::core::log_targets::BROWSING,
            "SIDEBAR_TRACE toggle_media_types_expanded end expanded={}",
            expanded
        );
        self.log_sidebar_layout_state("toggle_media_types_expanded_after");
        self.log_sidebar_layout_state_next_idle("toggle_media_types_expanded_after");
    }

    /// Rebuild the sidebar album rows from the current DB snapshot so counts
    /// stay live after favorites/trash changes.
    pub fn refresh_album_rows(&self) {
        tracing::debug!(
            target: crate::core::log_targets::BROWSING,
            "SIDEBAR_TRACE refresh_album_rows_sync_begin"
        );
        self.log_sidebar_layout_state("refresh_album_rows_sync_begin");
        self.update_photos_count_label_from_db();
        self.rebuild_album_rows();
        self.rebuild_media_type_rows();
        self.log_sidebar_layout_state("refresh_album_rows_sync_end");
        self.log_sidebar_layout_state_next_idle("refresh_album_rows_sync_end");
    }

    pub(super) fn update_photos_count_label(&self) {
        let count = self
            .imp()
            .media_list
            .borrow()
            .as_ref()
            .map(|list| list.n_items())
            .unwrap_or(0);
        self.set_photos_count_label(count);
    }

    fn set_photos_count_label(&self, count: u32) {
        let Some(label) = self.imp().photos_count_label.borrow().as_ref().cloned() else {
            return;
        };
        label.set_label(&count.to_string());
        label.set_visible(true);
    }

    pub(super) fn update_photos_count_label_from_db(&self) {
        let count = self
            .imp()
            .pool
            .borrow()
            .as_ref()
            .and_then(|pool| {
                crate::core::repository::MediaRepository::new(pool.clone())
                    .count(crate::core::repository::MediaQuery::LiveAll)
                    .ok()
            })
            .unwrap_or_else(|| {
                self.imp()
                    .media_list
                    .borrow()
                    .as_ref()
                    .map(|list| list.n_items())
                    .unwrap_or(0)
            });
        self.set_photos_count_label(count);
    }

    pub fn refresh_sidebar_snapshot_async(&self) {
        let Some(pool) = self.imp().pool.borrow().clone() else {
            return;
        };
        let trace_id = SIDEBAR_SNAPSHOT_TRACE_ID.fetch_add(1, Ordering::Relaxed);
        tracing::debug!(
            target: crate::core::log_targets::BROWSING,
            "SIDEBAR_TRACE snapshot_request id={} album_targets={} media_type_targets={}",
            trace_id,
            self.imp().album_targets.borrow().len(),
            self.imp().media_type_targets.borrow().len()
        );
        self.log_sidebar_layout_state(&format!("snapshot_request#{trace_id}"));
        self.log_sidebar_layout_state_next_idle(format!("snapshot_request#{trace_id}"));
        let weak = self.downgrade();
        glib::spawn_future_local(async move {
            let result = gtk::gio::spawn_blocking(move || {
                let snapshot = load_sidebar_album_snapshot(&pool);
                tracing::debug!(
                    target: crate::core::log_targets::BROWSING,
                    "SIDEBAR_TRACE snapshot_loaded id={} live_count={:?} albums={} media_types={} albums_summary=[{}] media_types_summary=[{}]",
                    trace_id,
                    snapshot.live_count,
                    snapshot.albums.len(),
                    snapshot.media_type_albums.len(),
                    sidebar_album_summary(&snapshot.albums),
                    sidebar_album_summary(&snapshot.media_type_albums)
                );
                snapshot
            })
            .await;
            let Some(window) = weak.upgrade() else {
                return;
            };
            match result {
                Ok(snapshot) => {
                    tracing::debug!(
                        target: crate::core::log_targets::BROWSING,
                        "SIDEBAR_TRACE snapshot_deliver id={}",
                        trace_id
                    );
                    window.apply_sidebar_album_snapshot(snapshot);
                    window.log_sidebar_layout_state(&format!("snapshot_deliver#{trace_id}_after"));
                    window.log_sidebar_layout_state_next_idle(format!(
                        "snapshot_deliver#{trace_id}_after"
                    ));
                }
                Err(err) => tracing::warn!("sidebar snapshot refresh failed: {err:?}"),
            }
        });
    }
}

#[cfg(test)]
mod tests;
