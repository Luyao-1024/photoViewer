use super::albums::album_initial_load_limit;
use super::{
    backfill_album_media_list, pop_to_photos_root, visible_page_is_trash, MainWindow, SidebarTarget,
};
use crate::core::albums::Album;
use crate::core::i18n::tr;
use crate::ui::album_detail_page::{media_query_for_album, AlbumDetailPage};
use crate::ui::{SearchPage, TrashPage};
use gtk4 as gtk;
use gtk4::subclass::prelude::ObjectSubclassIsExt;
use gtk4::{glib, prelude::*};
use libadwaita as adw;
use std::time::Instant;

impl MainWindow {
    /// Wire the sidebar `ListBox` row-selected signal to navigate by row
    /// identity (`targets[index]`), not a hardcoded index:
    ///   - Photos → return to the Photos child of the browsing stack.
    ///   - Trash → push the `TrashPage`.
    ///
    /// Album rows live in `album_list` and are wired separately below.
    ///
    /// The Albums header is non-selectable, so it never lands here; its collapse
    /// toggle is driven by its own `GestureClick`.
    ///
    /// Requires `set_resources` to have been called first; if the resources are
    /// missing the closures silently no-op.
    pub fn connect_sidebar(&self, nav_view: &adw::NavigationView) {
        self.install_sidebar_layout_trace();
        let list = self.imp().sidebar_list.get();
        let trash_list = self.imp().trash_list.get();
        let album_list = self.imp().album_list.get();
        let media_type_list = self.imp().media_type_list.get();

        list.connect_row_selected(
            glib::clone!(@weak self as window, @weak nav_view => move |_list, row| {
                let Some(row) = row else {
                    return;
                };
                if window.imp().selecting_programmatically.get() {
                    return;
                }
                let target = {
                    let targets = window.imp().targets.borrow();
                    let Some(target) = targets.get(row.index() as usize).cloned() else {
                        return;
                    };
                    target
                };
                match target {
                    SidebarTarget::Photos => {
                        *window.imp().active_album.borrow_mut() = None;
                        window.imp().album_list.get().unselect_all();
                        window.imp().media_type_list.get().unselect_all();
                        window.imp().trash_list.get().unselect_all();
                        pop_to_photos_root(&nav_view);
                        if let Some(photos) = window
                            .browsing_stack()
                            .child_by_name("photos")
                            .and_downcast::<crate::ui::PhotosPage>()
                        {
                            window.show_photos_browsing_page(&photos);
                        }
                    }
                    SidebarTarget::AlbumsHeader => {}
                    SidebarTarget::Trash => {}
                }
            }),
        );

        trash_list.connect_row_selected(
            glib::clone!(@weak self as window, @weak nav_view => move |_list, row| {
                let Some(row) = row else {
                    return;
                };
                if window.imp().selecting_programmatically.get() {
                    return;
                }
                let target = {
                    let targets = window.imp().trash_targets.borrow();
                    let Some(target) = targets.get(row.index() as usize).cloned() else {
                        return;
                    };
                    target
                };
                if let SidebarTarget::Trash = target {
                    let return_album = if window
                        .browsing_stack()
                        .visible_child_name()
                        .as_deref()
                        == Some("album")
                    {
                        window.imp().active_album.borrow().clone()
                    } else {
                        None
                    };
                    *window.imp().trash_return_album.borrow_mut() = return_album;
                    *window.imp().active_album.borrow_mut() = None;
                    window.imp().sidebar_list.get().unselect_all();
                    window.imp().album_list.get().unselect_all();
                    window.imp().media_type_list.get().unselect_all();
                    window.show_trash_page(&nav_view);
                }
            }),
        );

        album_list.connect_row_selected(
            glib::clone!(@weak self as window, @weak nav_view => move |_list, row| {
                if window.imp().album_selection_mode.get() {
                    window.sync_selected_album_paths();
                    return;
                }
                let Some(row) = row else {
                    return;
                };
                if window.imp().selecting_programmatically.get() {
                    return;
                }
                let album = {
                    let targets = window.imp().album_targets.borrow();
                    let Some(album) = targets.get(row.index() as usize).cloned() else {
                        return;
                    };
                    album
                };
                *window.imp().active_album.borrow_mut() = Some(album.folder_path.clone());
                window.imp().sidebar_list.get().unselect_all();
                window.imp().media_type_list.get().unselect_all();
                window.imp().trash_list.get().unselect_all();
                window.schedule_album_open_from_sidebar(&nav_view, album, "album_list", row.index());
            }),
        );

        media_type_list.connect_row_selected(
            glib::clone!(@weak self as window, @weak nav_view => move |_list, row| {
                let Some(row) = row else {
                    return;
                };
                if window.imp().selecting_programmatically.get() {
                    return;
                }
                let album = {
                    let targets = window.imp().media_type_targets.borrow();
                    let Some(album) = targets.get(row.index() as usize).cloned() else {
                        return;
                    };
                    album
                };
                *window.imp().active_album.borrow_mut() = Some(album.folder_path.clone());
                window.imp().sidebar_list.get().unselect_all();
                window.imp().album_list.get().unselect_all();
                window.imp().trash_list.get().unselect_all();
                window.schedule_album_open_from_sidebar(
                    &nav_view,
                    album,
                    "media_type_list",
                    row.index(),
                );
            }),
        );

        let settings_btn = self.imp().settings_button.get();
        settings_btn.connect_clicked(glib::clone!(@weak self as window => move |_| {
            window.show_settings_dialog();
        }));

        self.imp()
            .album_selection_cancel_btn
            .set_label(&tr("common.cancel"));
        self.imp()
            .album_selection_delete_btn
            .set_label(&tr("album.selection.delete_selected"));
        self.imp()
            .album_selection_delete_btn
            .get()
            .set_sensitive(false);

        self.imp().album_selection_cancel_btn.connect_clicked(
            glib::clone!(@weak self as window => move |_| {
                window.exit_album_selection_mode();
            }),
        );
        self.imp().album_selection_delete_btn.connect_clicked(
            glib::clone!(@weak self as window => move |_| {
                window.confirm_delete_selected_albums();
            }),
        );

        nav_view.connect_visible_page_notify(glib::clone!(@weak self as window => move |_| {
            let weak = window.downgrade();
            glib::idle_add_local_once(move || {
                if let Some(window) = weak.upgrade() {
                    window.sync_sidebar_selection_for_browsing_page();
                }
            });
        }));
        nav_view.connect_popped(glib::clone!(@weak self as window => move |_, _| {
            let weak = window.downgrade();
            glib::idle_add_local_once(move || {
                if let Some(window) = weak.upgrade() {
                    window.sync_sidebar_selection_for_browsing_page();
                }
            });
        }));
    }

    fn sync_sidebar_selection_for_browsing_page(&self) {
        if !self.browsing_root_is_visible() {
            return;
        }

        let is_album = self.browsing_stack().visible_child_name().as_deref() == Some("album");
        if is_album && self.imp().active_album.borrow().is_none() {
            if let Some(album) = self.imp().trash_return_album.borrow_mut().take() {
                *self.imp().active_album.borrow_mut() = Some(album);
            }
        }
        self.imp().selecting_programmatically.set(true);
        self.imp().trash_list.get().unselect_all();
        if is_album {
            self.imp().sidebar_list.get().unselect_all();
            self.imp().media_type_list.get().unselect_all();
            if let Some(active) = self.imp().active_album.borrow().clone() {
                if let Some(index) = self
                    .imp()
                    .media_type_targets
                    .borrow()
                    .iter()
                    .position(|album| album.folder_path == active)
                {
                    if let Some(row) = self.imp().media_type_list.get().row_at_index(index as i32) {
                        self.imp().media_type_list.get().select_row(Some(&row));
                    }
                } else if let Some(index) = self
                    .imp()
                    .album_targets
                    .borrow()
                    .iter()
                    .position(|album| album.folder_path == active)
                {
                    if let Some(row) = self.imp().album_list.get().row_at_index(index as i32) {
                        self.imp().album_list.get().select_row(Some(&row));
                    }
                }
            }
        } else {
            self.imp().active_album.borrow_mut().take();
            self.imp().album_list.get().unselect_all();
            self.imp().media_type_list.get().unselect_all();
            if let Some(row) = self.imp().sidebar_list.get().row_at_index(0) {
                self.imp().sidebar_list.get().select_row(Some(&row));
            }
        }
        self.imp().selecting_programmatically.set(false);
    }

    fn schedule_album_open_from_sidebar(
        &self,
        nav_view: &adw::NavigationView,
        album: Album,
        source: &'static str,
        row_index: i32,
    ) {
        let selected_at = Instant::now();
        let album_name = album.display_name();
        let album_path = album.folder_path.to_string_lossy().into_owned();
        let is_virtual = album.is_virtual;
        let expected_count = album.photo_count;
        let select_span = tracing::info_span!(
            "album:select_row",
            source,
            row_index,
            album_name = %album_name,
            album_path = %album_path,
            is_virtual,
            expected_count
        );
        let _select = select_span.enter();

        let weak = self.downgrade();
        let nav_view = nav_view.clone();
        glib::idle_add_local_once(move || {
            let Some(window) = weak.upgrade() else {
                return;
            };
            let idle_wait_ms = selected_at.elapsed().as_millis() as u64;
            let idle_span = tracing::info_span!(
                "album:open_idle",
                source,
                row_index,
                album_name = %album_name,
                album_path = %album_path,
                is_virtual,
                expected_count,
                idle_wait_ms
            );
            let _idle = idle_span.enter();
            window.open_album(&nav_view, album);
        });
    }

    #[tracing::instrument(name = "album:open", skip(self, nav_view, album))]
    pub(crate) fn open_album(&self, nav_view: &adw::NavigationView, album: Album) {
        let album_name = album.display_name();
        let album_path = album.folder_path.to_string_lossy().into_owned();
        let album_is_virtual = album.is_virtual;
        tracing::debug!(
            target: crate::core::log_targets::ALBUMS,
            album_name = %album_name,
            album_path = %album_path,
            is_virtual = album_is_virtual,
            expected_count = album.photo_count,
            "album_switch: begin"
        );

        let already_visible = {
            let check_span = tracing::info_span!(
                "album:already_visible_check",
                album_name = %album_name,
                album_path = %album_path
            );
            let _check = check_span.enter();
            self.visible_browsing_page()
                .and_then(|page| page.downcast::<AlbumDetailPage>().ok())
                .is_some_and(|detail| {
                    detail.album_folder_path().as_deref() == Some(album.folder_path.as_path())
                })
        };
        if already_visible {
            tracing::debug!(
                target: crate::core::log_targets::ALBUMS,
                album_name = %album_name,
                album_path = %album_path,
                "album_switch: already_visible"
            );
            return;
        }

        let Some(pool) = self.imp().pool.borrow().clone() else {
            tracing::warn!(
                target: crate::core::log_targets::ALBUMS,
                album_name = %album_name,
                album_path = %album_path,
                "album_switch: missing_db_pool"
            );
            return;
        };
        let Some(loader) = self.imp().loader.borrow().clone() else {
            tracing::warn!(
                target: crate::core::log_targets::ALBUMS,
                album_name = %album_name,
                album_path = %album_path,
                "album_switch: missing_thumbnail_loader"
            );
            return;
        };
        let Some(master) = self.imp().media_list.borrow().clone() else {
            tracing::warn!(
                target: crate::core::log_targets::ALBUMS,
                album_name = %album_name,
                album_path = %album_path,
                "album_switch: missing_master_media_list"
            );
            return;
        };

        let query = media_query_for_album(&album);
        let initial_limit = album_initial_load_limit(album.photo_count);
        let (items, total_items) = {
            let load_span = tracing::info_span!(
                "album:load",
                album_name = %album_name,
                album_path = %album_path,
                is_virtual = album_is_virtual,
                expected_count = album.photo_count,
                initial_limit,
                ?query
            );
            let _load = load_span.enter();
            match crate::core::repository::MediaRepository::new(pool.clone()).page(
                query.clone(),
                0,
                initial_limit,
            ) {
                Ok(page) => {
                    tracing::debug!(
                        target: crate::core::log_targets::ALBUMS,
                        album_name = %album_name,
                        album_path = %album_path,
                        is_virtual = album_is_virtual,
                        expected_count = album.photo_count,
                        initial_limit,
                        item_count = page.items.len(),
                        total_items = page.total,
                        "album_switch: initial_page_loaded"
                    );
                    (page.items, page.total)
                }
                Err(err) => {
                    tracing::warn!(
                        target: crate::core::log_targets::ALBUMS,
                        album_name = %album_name,
                        album_path = %album_path,
                        ?query,
                        "album_switch: initial_page_failed error={err}"
                    );
                    (Vec::new(), 0)
                }
            }
        };
        let item_count = items.len();

        let filtered = gtk::gio::ListStore::new::<glib::BoxedAnyObject>();
        {
            let store_span = tracing::info_span!(
                "album:store",
                album_name = %album_name,
                album_path = %album_path,
                item_count,
                total_items
            );
            let _store = store_span.enter();
            for item in items {
                filtered.append(&glib::BoxedAnyObject::new(item));
            }
        }

        let page = {
            let page_span = tracing::info_span!(
                "album:page_build",
                album_name = %album_name,
                album_path = %album_path,
                item_count,
                total_items
            );
            let _page = page_span.enter();
            AlbumDetailPage::new(album, filtered.clone(), master, pool.clone(), loader)
        };
        {
            let bind_span = tracing::info_span!(
                "album:bind_page",
                album_name = %album_name,
                album_path = %album_path
            );
            let _bind = bind_span.enter();
            if let Some(db_actor) = self.imp().db_actor.borrow().as_ref().cloned() {
                page.set_db_actor(db_actor);
            }
            page.set_nav_target(nav_view);
        }
        {
            let switch_span = tracing::info_span!(
                "album:switch",
                album_name = %album_name,
                album_path = %album_path,
                item_count,
                total_items
            );
            let _switch = switch_span.enter();
            self.show_album_browsing_page(&page);
        }

        tracing::debug!(
            target: crate::core::log_targets::ALBUMS,
            album_name = %album_name,
            album_path = %album_path,
            is_virtual = album_is_virtual,
            item_count,
            "album_switch: end"
        );

        if total_items > item_count as u32 {
            backfill_album_media_list(
                filtered,
                pool,
                query,
                item_count as u32,
                total_items,
                album_name,
                album_path,
            );
        }
    }

    fn show_trash_page(&self, nav_view: &adw::NavigationView) {
        if visible_page_is_trash(nav_view) {
            return;
        }
        let Some(page) = self.build_trash_page() else {
            return;
        };
        pop_to_photos_root(nav_view);
        nav_view.push(&page);
    }

    pub(super) fn open_search_page(&self) -> bool {
        let nav = self.imp().nav_view.get();
        if let Some(search) = nav
            .visible_page()
            .and_then(|page| page.downcast::<SearchPage>().ok())
        {
            search.focus_search_entry();
            return true;
        }
        let Some(pool) = self.imp().pool.borrow().as_ref().cloned() else {
            return false;
        };
        let Some(loader) = self.imp().loader.borrow().as_ref().cloned() else {
            return false;
        };
        let page = SearchPage::new(pool, loader);
        if let Some(db_actor) = self.imp().db_actor.borrow().as_ref().cloned() {
            page.set_db_actor(db_actor);
        }
        page.set_nav_target(&nav);
        nav.push(&page);
        true
    }

    /// 若当前可见页面是回收站页，重读 DB 刷新它。供 `TrashChanged` 事件调用——
    /// 文件管理器改了系统回收站后，watcher 已对账 DB，这里让打开着的回收站页实时
    /// 跟着变，无需用户切换页面。
    pub fn refresh_visible_trash_page(&self) {
        let nav = self.imp().nav_view.get();
        let Some(page) = nav.visible_page() else {
            return;
        };
        if let Some(trash) = page.downcast_ref::<TrashPage>() {
            trash.refresh();
        }
    }

    /// Refresh an already-open album detail page after live media membership
    /// changes. Album rows and counts refresh through the sidebar snapshot;
    /// this keeps the page's own filtered `ListStore` in sync while the user
    /// stays on that album.
    pub fn refresh_visible_album_detail_page(&self) {
        let nav = self.imp().nav_view.get();
        let Some(page) = nav.visible_page() else {
            return;
        };
        if let Some(album_detail) = page.downcast_ref::<AlbumDetailPage>() {
            album_detail.refresh_media_list_from_repository();
        }
    }
}

#[cfg(test)]
mod tests;
