//! A single-dialog album chooser with cover tiles and in-place Copy/Move actions.

use gtk4 as gtk;
use gtk4::glib;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use crate::core::album_ops::AlbumOpMode;
use crate::core::albums::{self, Album};
use crate::core::db::DbPool;
use crate::core::db_actor::DbActorHandle;
use crate::core::i18n::{tr, trf};
use crate::core::thumbnails::{ThumbnailLoader, ThumbnailSize, TIER_NORMAL};
use crate::ui::SquareTile;

pub struct AlbumPickerDialog;

impl AlbumPickerDialog {
    pub fn present(
        host_nav: &adw::NavigationView,
        pool: DbPool,
        db_actor: DbActorHandle,
        loader: Arc<ThumbnailLoader>,
        media_ids: Vec<i64>,
    ) {
        if media_ids.is_empty() {
            return;
        }

        let dialog = adw::Dialog::builder()
            .title(tr("album_picker.title"))
            .content_width(620)
            .content_height(560)
            .build();
        dialog.add_css_class("glass-alert-dialog");
        dialog.add_css_class("album-picker-dialog");

        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .build();
        let header = adw::HeaderBar::builder()
            .show_end_title_buttons(false)
            .css_classes(["glass-header"])
            .build();
        let cancel = gtk::Button::from_icon_name("window-close-symbolic");
        cancel.set_tooltip_text(Some(&tr("dialog.cancel")));
        cancel.add_css_class("glass-toolbar-button");
        cancel.add_css_class("round-search-button");
        let weak_dialog = dialog.downgrade();
        cancel.connect_clicked(move |_| {
            if let Some(dialog) = weak_dialog.upgrade() {
                dialog.close();
            }
        });
        header.pack_end(&cancel);
        content.append(&header);

        let grid = gtk::FlowBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .column_spacing(8)
            .row_spacing(8)
            .min_children_per_line(2)
            .max_children_per_line(4)
            .homogeneous(true)
            .valign(gtk::Align::Start)
            .build();
        grid.add_css_class("album-picker-grid");
        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .vexpand(true)
            .child(&grid)
            .build();
        content.append(&scroller);

        let status = gtk::Label::builder()
            .wrap(true)
            .xalign(0.0)
            .visible(false)
            .css_classes(["error", "album-picker-status"])
            .build();
        content.append(&status);

        let actions = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .halign(gtk::Align::End)
            .spacing(10)
            .css_classes(["album-picker-actions"])
            .build();
        let copy = gtk::Button::with_label(&tr("album_picker.copy"));
        copy.add_css_class("glass-toolbar-button");
        copy.add_css_class("glass-toolbar-suggested");
        copy.set_sensitive(false);
        let move_btn = gtk::Button::with_label(&tr("album_picker.move"));
        move_btn.add_css_class("glass-toolbar-button");
        move_btn.add_css_class("glass-toolbar-danger");
        move_btn.set_sensitive(false);
        actions.append(&copy);
        actions.append(&move_btn);
        content.append(&actions);
        dialog.set_child(Some(&content));

        let selected = Rc::new(RefCell::new(None::<PathBuf>));
        for (button, mode) in [(&copy, AlbumOpMode::Copy), (&move_btn, AlbumOpMode::Move)] {
            let selected = selected.clone();
            let pool = pool.clone();
            let db_actor = db_actor.clone();
            let ids = media_ids.clone();
            let host_nav = host_nav.downgrade();
            let dialog = dialog.downgrade();
            let copy = copy.downgrade();
            let move_btn = move_btn.downgrade();
            let status = status.downgrade();
            button.connect_clicked(move |_| {
                let Some(folder) = selected.borrow().clone() else {
                    return;
                };
                let (Some(copy), Some(move_btn), Some(status)) =
                    (copy.upgrade(), move_btn.upgrade(), status.upgrade())
                else {
                    return;
                };
                copy.set_sensitive(false);
                move_btn.set_sensitive(false);
                status.set_visible(false);
                let pool = pool.clone();
                let db_actor = db_actor.clone();
                let ids = ids.clone();
                let dialog = dialog.clone();
                let host_nav = host_nav.clone();
                let copy = copy.clone();
                let move_btn = move_btn.clone();
                let status = status.clone();
                glib::spawn_future_local(async move {
                    let result = tokio::task::spawn_blocking(move || {
                        crate::core::album_ops::add_to_album_with_actor(
                            &pool, &db_actor, &ids, &folder, mode,
                        )
                    })
                    .await;
                    match result {
                        Ok(Ok(_)) => {
                            if let Some(nav) = host_nav.upgrade() {
                                crate::ui::window::refresh_after_album_operation(&nav);
                            }
                            if let Some(dialog) = dialog.upgrade() {
                                dialog.close();
                            }
                        }
                        result => {
                            let message = match result {
                                Ok(Err(err)) => err.to_string(),
                                Err(err) => err.to_string(),
                                Ok(Ok(_)) => unreachable!(),
                            };
                            tracing::warn!("AlbumPicker: add_to_album failed: {message}");
                            status.set_label(&message);
                            status.set_visible(true);
                            copy.set_sensitive(true);
                            move_btn.set_sensitive(true);
                        }
                    }
                });
            });
        }

        dialog.present(host_nav);
        let pool_for_list = pool.clone();
        glib::spawn_future_local(async move {
            let result = gtk::gio::spawn_blocking(move || albums::list(&pool_for_list)).await;
            match result {
                Ok(Ok(albums)) if !albums.is_empty() => {
                    let selected_tile = Rc::new(RefCell::new(None::<glib::WeakRef<SquareTile>>));
                    for album in albums {
                        let (button, tile) = album_tile(&album, &loader);
                        let tile = tile.downgrade();
                        let folder = album.folder_path.clone();
                        let selected = selected.clone();
                        let selected_tile = selected_tile.clone();
                        let copy = copy.downgrade();
                        let move_btn = move_btn.downgrade();
                        button.connect_clicked(move |_| {
                            let Some(tile) = tile.upgrade() else {
                                return;
                            };
                            let (Some(copy), Some(move_btn)) = (copy.upgrade(), move_btn.upgrade())
                            else {
                                return;
                            };
                            if let Some(previous) =
                                selected_tile.borrow_mut().replace(tile.downgrade())
                            {
                                if let Some(previous) = previous.upgrade() {
                                    previous.remove_css_class("media-selected");
                                }
                            }
                            tile.add_css_class("media-selected");
                            *selected.borrow_mut() = Some(folder.clone());
                            copy.set_sensitive(true);
                            move_btn.set_sensitive(true);
                        });
                        grid.insert(&button, -1);
                    }
                }
                Ok(Ok(_)) => {
                    status.set_label(&tr("album_picker.no_albums_yet.description"));
                    status.set_visible(true);
                }
                error => {
                    tracing::warn!("AlbumPicker: album listing failed: {error:?}");
                    status.set_label(&tr("album_picker.no_albums_yet.title"));
                    status.set_visible(true);
                }
            }
        });
    }
}

fn album_tile(album: &Album, loader: &Arc<ThumbnailLoader>) -> (gtk::Button, SquareTile) {
    let body = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(5)
        .build();
    let cover = gtk::Overlay::new();
    cover.add_css_class("album-picker-cover");
    let tile = SquareTile::new();
    tile.set_target(120);
    tile.set_height_for_width(true);
    tile.set_allow_width_shrink(true);
    cover.set_child(Some(&tile));
    let fallback = gtk::Image::from_icon_name("folder-pictures-symbolic");
    fallback.set_pixel_size(36);
    fallback.set_halign(gtk::Align::Center);
    fallback.set_valign(gtk::Align::Center);
    cover.add_overlay(&fallback);
    body.append(&cover);
    let title = gtk::Label::builder()
        .label(album.display_name())
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .max_width_chars(16)
        .build();
    body.append(&title);
    let count = gtk::Label::builder()
        .label(trf(
            "album.count",
            &[("count", &album.photo_count.to_string())],
        ))
        .css_classes(["dim-label"])
        .build();
    body.append(&count);
    let button = gtk::Button::builder()
        .child(&body)
        .tooltip_text(album.folder_path.display().to_string())
        .css_classes(["album-picker-tile"])
        .build();

    if let Some(uri) = &album.cover_uri {
        let (tx, rx) = tokio::sync::oneshot::channel();
        loader.request(
            uri.clone(),
            ThumbnailSize::Small,
            Some(std::time::SystemTime::from(album.last_modified)),
            tx,
            TIER_NORMAL,
        );
        let tile = tile.downgrade();
        let fallback = fallback.downgrade();
        glib::spawn_future_local(async move {
            if let Ok(loaded) = rx.await {
                if let Some(tile) = tile.upgrade() {
                    tile.set_paintable(Some(&loaded.texture));
                }
                if let Some(fallback) = fallback.upgrade() {
                    fallback.set_visible(false);
                }
            }
        });
    }
    (button, tile)
}
