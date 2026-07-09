use super::settings::{add_excluded_scan_path, show_settings_error_dialog};
use super::sidebar::sidebar_album_summary;
use super::{pop_to_photos_root, show_trash_operation_error_dialog, MainWindow};
use crate::core::albums::set_album_order;
use crate::core::albums::Album;
use crate::core::db::DbPool;
use crate::core::i18n::{tr, trf};
use crate::core::media::MediaItem;
use crate::core::prefs;
use crate::core::prefs::TrashBackend;
use crate::core::repository::MediaMutation;
use crate::ui::glass_context_menu::{self, GlassMenuItem, GlassMenuItemKind};
use gtk4 as gtk;
use gtk4::subclass::prelude::ObjectSubclassIsExt;
use gtk4::{glib, prelude::*};
use libadwaita as adw;
use libadwaita::prelude::*;
use std::collections::HashSet;
use std::path::PathBuf;

impl MainWindow {
    pub fn enter_album_selection_mode(&self) {
        self.imp().album_selection_mode.set(true);
        self.imp()
            .album_list
            .get()
            .set_selection_mode(gtk::SelectionMode::Multiple);
        self.imp().album_list.get().unselect_all();
        self.imp().selected_album_paths.borrow_mut().clear();
        self.imp().album_selection_bar.get().set_revealed(true);
        self.update_album_selection_actions();
    }

    pub(super) fn exit_album_selection_mode(&self) {
        self.imp().album_selection_mode.set(false);
        self.imp().album_list.get().unselect_all();
        self.imp()
            .album_list
            .get()
            .set_selection_mode(gtk::SelectionMode::Single);
        self.imp().selected_album_paths.borrow_mut().clear();
        self.imp().album_selection_bar.get().set_revealed(false);
        self.update_album_selection_actions();
    }

    pub fn selected_album_delete_count(&self) -> usize {
        self.imp().selected_album_paths.borrow().len()
    }

    pub(super) fn sync_selected_album_paths(&self) {
        let album_list = self.imp().album_list.get();
        let targets = self.imp().album_targets.borrow().clone();
        let mut selected = HashSet::new();
        let mut virtual_rows = Vec::new();

        for row in album_list.selected_rows() {
            let Some(album) = targets.get(row.index() as usize) else {
                continue;
            };
            if album.is_virtual {
                virtual_rows.push(row);
            } else {
                selected.insert(album.folder_path.clone());
            }
        }

        for row in virtual_rows {
            album_list.unselect_row(&row);
        }

        *self.imp().selected_album_paths.borrow_mut() = selected;
        self.update_album_selection_actions();
    }

    fn update_album_selection_actions(&self) {
        self.imp()
            .album_selection_delete_btn
            .get()
            .set_sensitive(self.selected_album_delete_count() > 0);
    }

    fn selected_real_albums(&self) -> Vec<Album> {
        let selected = self.imp().selected_album_paths.borrow().clone();
        self.imp()
            .album_targets
            .borrow()
            .iter()
            .filter(|album| !album.is_virtual && selected.contains(&album.folder_path))
            .cloned()
            .collect()
    }

    /// Persist a drag-to-reorder: move the album at `source_path` so it lands
    /// just before (`drop_after = false`) or just after (`drop_after = true`)
    /// the album at `target_path`, then rebuild the rows so the sidebar matches.
    ///
    /// The new full order is derived from the currently displayed `targets`
    /// (the source of truth for what the user sees), written wholesale to
    /// `album_order`, then `rebuild_album_rows` re-fetches and re-applies it.
    fn reorder_album(&self, source_path: &str, target_path: &str, drop_after: bool) {
        if source_path == target_path {
            return;
        }
        let Some(pool) = self.imp().pool.borrow().clone() else {
            return;
        };

        let mut order: Vec<String> = self
            .imp()
            .album_targets
            .borrow()
            .iter()
            .map(|album| album.folder_path.to_string_lossy().into_owned())
            .filter(|p| p != source_path)
            .collect();

        let insert_at = match order.iter().position(|p| p == target_path) {
            Some(idx) => {
                if drop_after {
                    (idx + 1).min(order.len())
                } else {
                    idx
                }
            }
            None => order.len(),
        };
        order.insert(insert_at, source_path.to_string());

        if let Err(err) = set_album_order(&pool, &order) {
            tracing::warn!("failed to persist album order: {err}");
        }
        self.rebuild_album_rows();
    }

    /// Wire long-press-drag reorder onto an album row: a `DragSource` carries
    /// the row's `folder_path` as the drag payload (and dims the row while
    /// dragging), and a `DropTarget` accepts another album's path, showing an
    /// above/below insertion indicator and persisting the new order on drop.
    ///
    /// `Gtk.DragSource` only begins a drag after the pointer moves past the
    /// drag threshold, so a plain click still selects the row normally - only
    /// a press-and-drag reorders.
    pub(super) fn attach_album_dnd(&self, row: &gtk::ListBoxRow, folder_path: String) {
        let drag = gtk::DragSource::new();
        drag.set_actions(gtk::gdk::DragAction::MOVE);
        let value = glib::Value::from(folder_path.as_str());
        drag.set_content(Some(&gtk::gdk::ContentProvider::for_value(&value)));

        let drag_row = row.downgrade();
        drag.connect_drag_begin(move |_, _| {
            if let Some(r) = drag_row.upgrade() {
                r.add_css_class("glass-sidebar-row-dragging");
            }
        });
        let drag_row = row.downgrade();
        drag.connect_drag_end(move |_, _, _| {
            if let Some(r) = drag_row.upgrade() {
                r.remove_css_class("glass-sidebar-row-dragging");
            }
        });
        row.add_controller(drag);

        let drop = gtk::DropTarget::new(glib::Type::STRING, gtk::gdk::DragAction::MOVE);

        let motion_row = row.downgrade();
        drop.connect_motion(move |_t, _x, y| {
            if let Some(r) = motion_row.upgrade() {
                let half = r.height().max(1) as f64 / 2.0;
                r.remove_css_class("glass-sidebar-row-drop-above");
                r.remove_css_class("glass-sidebar-row-drop-below");
                r.add_css_class(if y > half {
                    "glass-sidebar-row-drop-below"
                } else {
                    "glass-sidebar-row-drop-above"
                });
            }
            gtk::gdk::DragAction::MOVE
        });
        let leave_row = row.downgrade();
        drop.connect_leave(move |_t| {
            if let Some(r) = leave_row.upgrade() {
                r.remove_css_class("glass-sidebar-row-drop-above");
                r.remove_css_class("glass-sidebar-row-drop-below");
            }
        });

        let weak = self.downgrade();
        let drop_row = row.downgrade();
        let target_path = folder_path;
        drop.connect_drop(move |_t, value, _x, y| {
            let Some(window) = weak.upgrade() else {
                return false;
            };
            let Some(r) = drop_row.upgrade() else {
                return false;
            };
            r.remove_css_class("glass-sidebar-row-drop-above");
            r.remove_css_class("glass-sidebar-row-drop-below");
            let Ok(src) = value.get::<String>() else {
                return false;
            };
            let half = r.height().max(1) as f64 / 2.0;
            window.reorder_album(&src, &target_path, y > half);
            true
        });
        row.add_controller(drop);
    }

    pub(super) fn attach_album_context_menu(&self, row: &gtk::ListBoxRow, album: Album) {
        let weak = self.downgrade();
        let row_weak = row.downgrade();
        let gesture = gtk::GestureClick::new();
        gesture.set_button(3);
        gesture.connect_pressed(move |_gesture, n_press, x, y| {
            if n_press != 1 {
                return;
            }
            let Some(window) = weak.upgrade() else {
                return;
            };
            let Some(row) = row_weak.upgrade() else {
                return;
            };

            let manage_album = album.clone();
            let delete_album = album.clone();
            let ignore_album = album.clone();
            let select_album = album.clone();
            let nav_view = window.imp().nav_view.get();
            let items = build_album_context_menu_items(
                &album,
                Some(Box::new(glib::clone!(
                    @weak window,
                    @weak nav_view,
                    @weak row,
                    @strong manage_album => move || {
                        *window.imp().active_album.borrow_mut() =
                            Some(manage_album.folder_path.clone());
                        window.imp().selecting_programmatically.set(true);
                        window.imp().album_list.get().select_row(Some(&row));
                        window.imp().selecting_programmatically.set(false);
                        window.imp().sidebar_list.get().unselect_all();
                        window.imp().media_type_list.get().unselect_all();
                        window.imp().trash_list.get().unselect_all();
                        window.open_album(&nav_view, manage_album.clone());
                    }
                ))),
                Some(Box::new(glib::clone!(
                    @weak window,
                    @strong delete_album => move || {
                        window.confirm_delete_album(delete_album.clone());
                    }
                ))),
                Some(Box::new(glib::clone!(
                    @weak window,
                    @strong ignore_album => move || {
                        window.confirm_ignore_album(ignore_album.clone());
                    }
                ))),
                Some(Box::new(glib::clone!(
                    @weak window,
                    @weak row,
                    @strong select_album => move || {
                        window.enter_album_selection_mode();
                        if !select_album.is_virtual {
                            window.imp().album_list.get().select_row(Some(&row));
                        }
                    }
                ))),
            );
            glass_context_menu::show(
                &window.imp().root_overlay.get(),
                row.upcast_ref(),
                x,
                y,
                items,
            );
        });
        row.add_controller(gesture);
    }

    fn confirm_delete_album(&self, album: Album) {
        if album.is_virtual {
            return;
        }

        let album_name = album.display_name();
        let dialog = adw::AlertDialog::builder()
            .heading(tr("album.delete.confirm_title"))
            .body(trf("album.delete.confirm_body", &[("album", &album_name)]))
            .build();
        dialog.add_css_class("glass-alert-dialog");
        dialog.add_response("cancel", &tr("common.cancel"));
        dialog.add_response("delete", &tr("album.delete.confirm_action"));
        dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("cancel"));
        dialog.set_close_response("cancel");

        let weak = self.downgrade();
        dialog.connect_response(Some("delete"), move |_, _| {
            if let Some(window) = weak.upgrade() {
                window.delete_albums_to_trash_ui(vec![album.clone()]);
            }
        });

        dialog.present(self);
    }

    fn confirm_ignore_album(&self, album: Album) {
        if album.is_virtual {
            return;
        }

        let album_name = album.display_name();
        let dialog = adw::AlertDialog::builder()
            .heading(tr("album.ignore.confirm_title"))
            .body(trf("album.ignore.confirm_body", &[("album", &album_name)]))
            .build();
        dialog.add_css_class("glass-alert-dialog");
        dialog.add_response("cancel", &tr("common.cancel"));
        dialog.add_response("ignore", &tr("album.ignore.confirm_action"));
        dialog.set_response_appearance("ignore", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("cancel"));
        dialog.set_close_response("cancel");

        let weak = self.downgrade();
        dialog.connect_response(Some("ignore"), move |_, _| {
            if let Some(window) = weak.upgrade() {
                window.ignore_album_ui(album.clone());
            }
        });

        dialog.present(self);
    }

    fn ignore_album_ui(&self, album: Album) {
        if album.is_virtual {
            return;
        }
        let Some(pool) = self.imp().pool.borrow().clone() else {
            return;
        };

        let weak = self.downgrade();
        glib::spawn_future_local(async move {
            let worker_result =
                gtk::gio::spawn_blocking(move || ignore_album_worker(pool, album.folder_path))
                    .await;

            let Some(window) = weak.upgrade() else {
                return;
            };
            match worker_result {
                Ok(Ok(result)) => {
                    tracing::info!(
                        "ignored album {} and removed {} indexed media rows",
                        result.folder_path.display(),
                        result.removed_count
                    );
                    if let Some(media_list) = window.imp().media_list.borrow().as_ref() {
                        remove_ignored_album_media_from_media_list(media_list, &result.folder_path);
                    }
                    window.refresh_album_rows();

                    let active_should_close = window
                        .imp()
                        .active_album
                        .borrow()
                        .as_ref()
                        .is_some_and(|active| active == &result.folder_path);
                    if active_should_close {
                        *window.imp().active_album.borrow_mut() = None;
                        window.imp().album_list.get().unselect_all();
                        window.imp().trash_list.get().unselect_all();
                        window.imp().selecting_programmatically.set(true);
                        if let Some(row) = window.imp().sidebar_list.get().row_at_index(0) {
                            window.imp().sidebar_list.get().select_row(Some(&row));
                        }
                        window.imp().selecting_programmatically.set(false);
                        pop_to_photos_root(&window.imp().nav_view.get());
                    }
                }
                Ok(Err(err)) => {
                    tracing::warn!("failed to ignore album: {err}");
                    show_settings_error_dialog(
                        window.upcast_ref(),
                        &trf("setting.scan_paths.save_failed", &[("error", &err)]),
                    );
                    window.refresh_album_rows();
                }
                Err(err) => {
                    tracing::warn!("album ignore worker failed: {err:?}");
                    window.refresh_album_rows();
                }
            }
        });
    }

    pub(super) fn confirm_delete_selected_albums(&self) {
        let selected = self.selected_real_albums();
        if selected.is_empty() {
            return;
        }

        let count = selected.len().to_string();
        let dialog = adw::AlertDialog::builder()
            .heading(tr("album.selection.confirm_title"))
            .body(trf("album.selection.confirm_body", &[("count", &count)]))
            .build();
        dialog.add_css_class("glass-alert-dialog");
        dialog.add_response("cancel", &tr("common.cancel"));
        dialog.add_response("delete", &tr("album.delete.confirm_action"));
        dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("cancel"));
        dialog.set_close_response("cancel");

        let weak = self.downgrade();
        dialog.connect_response(Some("delete"), move |_, _| {
            if let Some(window) = weak.upgrade() {
                window.delete_albums_to_trash_ui(selected.clone());
                window.exit_album_selection_mode();
            }
        });

        dialog.present(self);
    }

    fn delete_albums_to_trash_ui(&self, albums: Vec<Album>) {
        if albums.is_empty() {
            return;
        }
        let Some(pool) = self.imp().pool.borrow().clone() else {
            return;
        };
        tracing::debug!(
            target: crate::core::log_targets::BROWSING,
            "SIDEBAR_TRACE delete_albums_to_trash_ui_begin albums={} summary=[{}]",
            albums.len(),
            sidebar_album_summary(&albums)
        );
        self.log_sidebar_layout_state("delete_albums_to_trash_ui_begin");

        let weak = self.downgrade();
        let retry_pool = pool.clone();
        let retry_albums = albums.clone();
        glib::spawn_future_local(async move {
            let worker_result =
                gtk::gio::spawn_blocking(move || delete_albums_to_trash_worker(pool, albums)).await;

            let Some(window) = weak.upgrade() else {
                return;
            };
            match worker_result {
                Ok(result) => {
                    let operation_error = result.operation.as_ref().err().cloned();
                    window.apply_album_delete_ui_result(&result);
                    if let Some(err) = operation_error {
                        tracing::warn!("failed to delete album to trash: {err}");
                        if prefs::trash_backend() == TrashBackend::System {
                            window.prompt_album_trash_backend_fallback(
                                retry_pool,
                                retry_albums,
                                err,
                            );
                        } else {
                            show_trash_operation_error_dialog(
                                window.upcast_ref(),
                                &tr("trash.move_failed"),
                            );
                        }
                    }
                }
                Err(err) => {
                    tracing::warn!("album delete worker failed: {err:?}");
                    window.refresh_album_rows();
                }
            }
        });
    }

    fn apply_album_delete_ui_result(&self, result: &AlbumDeleteUiResult) {
        tracing::debug!(
            target: crate::core::log_targets::BROWSING,
            "SIDEBAR_TRACE apply_album_delete_result begin deleted_paths={} remaining_live_uris={} remaining_live_folders={} unknown_remaining_live_paths={}",
            result.deleted_paths.len(),
            result.remaining_live_uris.len(),
            result.remaining_live_folder_paths.len(),
            result.unknown_remaining_live_paths.len()
        );
        self.log_sidebar_layout_state("apply_album_delete_result_before_media_remove");
        if let Some(media_list) = self.imp().media_list.borrow().as_ref() {
            remove_deleted_album_media_from_media_list(
                media_list,
                &result.deleted_paths,
                &result.remaining_live_uris,
                &result.unknown_remaining_live_paths,
            );
        }
        self.log_sidebar_layout_state("apply_album_delete_result_before_refresh_album_rows");
        self.refresh_album_rows();

        let active_should_close = self
            .imp()
            .active_album
            .borrow()
            .as_ref()
            .is_some_and(|active| {
                result.deleted_paths.iter().any(|path| path == active)
                    && !result
                        .remaining_live_folder_paths
                        .iter()
                        .any(|path| path == active)
            });
        tracing::debug!(
            target: crate::core::log_targets::BROWSING,
            "SIDEBAR_TRACE apply_album_delete_result active_should_close={}",
            active_should_close
        );
        if active_should_close {
            *self.imp().active_album.borrow_mut() = None;
            self.imp().album_list.get().unselect_all();
            self.imp().trash_list.get().unselect_all();
            self.imp().selecting_programmatically.set(true);
            if let Some(row) = self.imp().sidebar_list.get().row_at_index(0) {
                self.imp().sidebar_list.get().select_row(Some(&row));
            }
            self.imp().selecting_programmatically.set(false);
            pop_to_photos_root(&self.imp().nav_view.get());
        }
        self.log_sidebar_layout_state("apply_album_delete_result_end");
        self.log_sidebar_layout_state_next_idle("apply_album_delete_result_end");
    }

    fn prompt_album_trash_backend_fallback(&self, pool: DbPool, albums: Vec<Album>, error: String) {
        let dialog = adw::AlertDialog::builder()
            .heading(tr("trash.fallback.title"))
            .body(trf("trash.fallback.album_body", &[("error", &error)]))
            .build();
        dialog.add_css_class("glass-alert-dialog");
        dialog.add_response("cancel", &tr("dialog.cancel"));
        dialog.add_response("switch", &tr("trash.fallback.switch_to_app"));
        dialog.set_response_appearance("switch", adw::ResponseAppearance::Suggested);
        dialog.set_default_response(Some("switch"));
        dialog.set_close_response("cancel");

        let weak = self.downgrade();
        dialog.connect_response(Some("switch"), move |_, _| {
            let pool = pool.clone();
            let albums = albums.clone();
            let weak = weak.clone();
            glib::spawn_future_local(async move {
                let worker_result = gtk::gio::spawn_blocking(move || {
                    crate::core::trash::switch_trash_backend(&pool, TrashBackend::App)
                        .map_err(|err| err.to_string())?;
                    Ok::<_, String>(delete_albums_to_trash_worker(pool, albums))
                })
                .await;

                let Some(window) = weak.upgrade() else {
                    return;
                };
                match worker_result {
                    Ok(Ok(result)) => {
                        let operation_error = result.operation.as_ref().err().cloned();
                        window.apply_album_delete_ui_result(&result);
                        if let Some(err) = operation_error {
                            tracing::warn!("failed to retry album trash delete: {err}");
                            show_trash_operation_error_dialog(
                                window.upcast_ref(),
                                &tr("trash.move_failed"),
                            );
                        }
                    }
                    Ok(Err(err)) => {
                        show_trash_operation_error_dialog(
                            window.upcast_ref(),
                            &trf("trash.fallback.switch_failed", &[("error", &err)]),
                        );
                    }
                    Err(err) => {
                        show_trash_operation_error_dialog(
                            window.upcast_ref(),
                            &trf(
                                "trash.fallback.switch_failed",
                                &[("error", &format!("{err:?}"))],
                            ),
                        );
                    }
                }
            });
        });

        dialog.present(self);
    }
}

pub(super) fn album_initial_load_limit(total: i64) -> u32 {
    let total = usize::try_from(total.max(0)).unwrap_or(usize::MAX);
    let plan = crate::core::runtime_config::progressive_render_plan(
        crate::core::runtime_config::startup_progressive_render(),
        crate::core::runtime_config::startup_render_seed(),
        total,
        crate::core::runtime_config::max_rendered_grid_items(),
    );
    u32::try_from(plan.model_limit).unwrap_or(u32::MAX)
}

pub(super) fn album_backfill_fetch_limit(current_len: u32, total: u32) -> u32 {
    if current_len >= total {
        return 0;
    }
    let ui_cap =
        u32::try_from(crate::core::runtime_config::ui_media_list_cap()).unwrap_or(u32::MAX);
    if current_len >= ui_cap {
        return 0;
    }
    total.saturating_sub(current_len).min(ui_cap - current_len)
}

pub(super) fn ignore_album_worker(
    pool: DbPool,
    folder_path: PathBuf,
) -> std::result::Result<AlbumIgnoreUiResult, String> {
    add_excluded_scan_path(folder_path.clone())?;
    let removed_count = crate::core::db::delete_live_media_by_folder(&pool, &folder_path)
        .map_err(|err| err.to_string())?;
    crate::core::albums::refresh(&pool).map_err(|err| err.to_string())?;
    Ok(AlbumIgnoreUiResult {
        folder_path,
        removed_count,
    })
}

pub(super) fn delete_albums_to_trash_worker(
    pool: DbPool,
    albums: Vec<Album>,
) -> AlbumDeleteUiResult {
    let deleted_paths = albums
        .iter()
        .filter(|album| !album.is_virtual)
        .map(|album| album.folder_path.clone())
        .collect::<Vec<_>>();
    let operation = crate::core::album_ops::delete_albums_to_trash(&pool, &albums)
        .map_err(|err| err.to_string());

    let mut remaining_live_uris = HashSet::new();
    let mut remaining_live_folder_paths = HashSet::new();
    let mut unknown_remaining_live_paths = HashSet::new();
    for path in &deleted_paths {
        match crate::core::db::list_media_by_folder(&pool, path) {
            Ok(items) => {
                if !items.is_empty() {
                    remaining_live_folder_paths.insert(path.clone());
                }
                remaining_live_uris.extend(items.into_iter().map(|item| item.uri));
            }
            Err(err) => {
                tracing::warn!(
                    "failed to query remaining live media for album {}: {err}",
                    path.display()
                );
                remaining_live_folder_paths.insert(path.clone());
                unknown_remaining_live_paths.insert(path.clone());
            }
        }
    }

    AlbumDeleteUiResult {
        operation,
        deleted_paths,
        remaining_live_uris,
        remaining_live_folder_paths,
        unknown_remaining_live_paths,
    }
}

pub fn build_album_context_menu_for_tests(album: &Album) -> gtk::Box {
    glass_context_menu::build_menu_panel_for_tests(build_album_context_menu_items(
        album, None, None, None, None,
    ))
}

pub(super) fn build_album_context_menu_items(
    album: &Album,
    on_manage: Option<Box<dyn Fn() + 'static>>,
    on_delete: Option<Box<dyn Fn() + 'static>>,
    on_ignore: Option<Box<dyn Fn() + 'static>>,
    on_select: Option<Box<dyn Fn() + 'static>>,
) -> Vec<GlassMenuItem> {
    let mut items = Vec::new();

    items.push(GlassMenuItem::new(
        tr("album.context.manage"),
        GlassMenuItemKind::Normal,
        move || {
            if let Some(on_manage) = &on_manage {
                on_manage();
            }
        },
    ));

    if let Some(on_select) = on_select {
        items.push(GlassMenuItem::new(
            tr("album.context.multi_select"),
            GlassMenuItemKind::Suggested,
            move || {
                on_select();
            },
        ));
    }

    if !album.is_virtual {
        items.push(GlassMenuItem::new(
            tr("album.context.ignore"),
            GlassMenuItemKind::Normal,
            move || {
                if let Some(on_ignore) = &on_ignore {
                    on_ignore();
                }
            },
        ));
        items.push(GlassMenuItem::new(
            tr("album.context.delete"),
            GlassMenuItemKind::Danger,
            move || {
                if let Some(on_delete) = &on_delete {
                    on_delete();
                }
            },
        ));
    }

    items
}

pub(crate) fn refresh_after_album_operation(nav: &adw::NavigationView) {
    if let Some(window) = nav
        .ancestor(MainWindow::static_type())
        .and_downcast::<MainWindow>()
    {
        let visible_page_type = nav
            .visible_page()
            .map(|page| page.type_().name().to_string())
            .unwrap_or_else(|| "<none>".to_string());
        tracing::debug!(
            target: crate::core::log_targets::ALBUMS,
            "PHOTO_REFRESH_TRACE refresh_after_album_operation visible_page_type={}",
            visible_page_type
        );
        window.refresh_shared_media_list_from_repository();
        window.refresh_visible_album_detail_page();
        window.refresh_sidebar_snapshot_async();
    }
}

pub(super) struct AlbumDeleteUiResult {
    operation: std::result::Result<MediaMutation, String>,
    deleted_paths: Vec<PathBuf>,
    remaining_live_uris: HashSet<String>,
    remaining_live_folder_paths: HashSet<PathBuf>,
    unknown_remaining_live_paths: HashSet<PathBuf>,
}

pub(super) struct AlbumIgnoreUiResult {
    folder_path: PathBuf,
    removed_count: usize,
}

pub(super) fn remove_ignored_album_media_from_media_list(
    media_list: &gtk::gio::ListStore,
    ignored_path: &PathBuf,
) {
    let mut index = 0;
    while index < media_list.n_items() {
        let should_remove = media_list
            .item(index)
            .and_downcast::<glib::BoxedAnyObject>()
            .is_some_and(|boxed| {
                let item = boxed.borrow::<MediaItem>();
                item.folder_path == *ignored_path
            });
        if should_remove {
            media_list.remove(index);
        } else {
            index += 1;
        }
    }
}

pub(super) fn remove_deleted_album_media_from_media_list(
    media_list: &gtk::gio::ListStore,
    deleted_paths: &[PathBuf],
    remaining_live_uris: &HashSet<String>,
    unknown_remaining_live_paths: &HashSet<PathBuf>,
) {
    if deleted_paths.is_empty() {
        return;
    }

    let deleted_paths: HashSet<&PathBuf> = deleted_paths.iter().collect();
    let mut index = 0;
    while index < media_list.n_items() {
        let should_remove = media_list
            .item(index)
            .and_downcast::<glib::BoxedAnyObject>()
            .is_some_and(|boxed| {
                let item = boxed.borrow::<MediaItem>();
                deleted_paths.contains(&item.folder_path)
                    && !unknown_remaining_live_paths.contains(&item.folder_path)
                    && !remaining_live_uris.contains(&item.uri)
            });
        if should_remove {
            media_list.remove(index);
        } else {
            index += 1;
        }
    }
}
