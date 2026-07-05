//! AlbumDetailPage — single-album day-grouped photo grid view.
use std::collections::HashSet;
use std::sync::Arc;

use gtk4 as gtk;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::NavigationPageExt;
use libadwaita::subclass::prelude::*;

use crate::core::albums::{self, Album};
use crate::core::db::DbPool;
use crate::core::db_actor::{DbActorHandle, DbCommand};
use crate::core::identity::MediaId;
use crate::core::media::MediaItem;
use crate::core::repository::{MediaQuery, MediaRepository};
use crate::core::section_model::GroupBy;
use crate::core::thumbnails::ThumbnailLoader;
use crate::ui::empty_states;
use crate::ui::media_grid::{FavoriteMenuState, MediaGrid, MediaGridCallbacks};
use crate::ui::viewer_page::{NavDelta, ViewerPage, NAV_POP, VIEWER_OPEN_POP_GUARD_MS};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(file = "../../data/ui/album-detail-page.ui")]
    pub struct AlbumDetailPage {
        pub media_list: RefCell<Option<gtk::gio::ListStore>>,
        pub master_media_list: RefCell<Option<gtk::gio::ListStore>>,
        pub pool: RefCell<Option<DbPool>>,
        pub db_actor: RefCell<Option<DbActorHandle>>,
        pub album: RefCell<Option<Album>>,
        pub nav_view: RefCell<Option<adw::NavigationView>>,
        pub loader: RefCell<Option<Arc<ThumbnailLoader>>>,
        /// Debounces media activation while NavigationView is pushing the
        /// viewer, matching PhotosPage's grid behavior.
        pub viewer_open_pending: Cell<bool>,
        #[template_child]
        pub header_bar: TemplateChild<adw::HeaderBar>,
        #[template_child]
        pub search_btn: TemplateChild<gtk::Button>,
        #[template_child]
        pub grid_overlay: TemplateChild<gtk::Overlay>,
        #[template_child]
        pub content_box: TemplateChild<gtk::Box>,
    }

    #[gtk::glib::object_subclass]
    impl ObjectSubclass for AlbumDetailPage {
        const NAME: &'static str = "AlbumDetailPage";
        type Type = super::AlbumDetailPage;
        type ParentType = adw::NavigationPage;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &gtk::glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for AlbumDetailPage {}
    impl WidgetImpl for AlbumDetailPage {}
    impl NavigationPageImpl for AlbumDetailPage {}
}

gtk::glib::wrapper! {
    pub struct AlbumDetailPage(ObjectSubclass<imp::AlbumDetailPage>)
        @extends adw::NavigationPage, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable;
}

impl AlbumDetailPage {
    /// Build an `AlbumDetailPage` populated with a pre-filtered media list.
    /// The grid uses the same `MediaGrid` Day grouping as `PhotosPage`.
    #[tracing::instrument(
        name = "album_detail:new",
        skip(album, media_list, master_media_list, pool, loader)
    )]
    pub fn new(
        album: Album,
        media_list: gtk::gio::ListStore,
        master_media_list: gtk::gio::ListStore,
        pool: DbPool,
        loader: Arc<ThumbnailLoader>,
    ) -> Self {
        let album_name = album.display_name();
        let album_path = album.folder_path.to_string_lossy().into_owned();
        let initial_items = media_list.n_items();
        tracing::debug!(
            target: crate::core::log_targets::ALBUMS,
            album_name = %album_name,
            album_path = %album_path,
            is_virtual = album.is_virtual,
            item_count = initial_items,
            "album_detail_page: build_begin"
        );

        let obj: Self = glib::Object::builder().build();
        obj.set_title(&album.display_name());
        *obj.imp().media_list.borrow_mut() = Some(media_list.clone());
        *obj.imp().master_media_list.borrow_mut() = Some(master_media_list);
        *obj.imp().album.borrow_mut() = Some(album);
        *obj.imp().pool.borrow_mut() = Some(pool);
        *obj.imp().loader.borrow_mut() = Some(loader.clone());

        if media_list.n_items() == 0 {
            let empty_span = tracing::info_span!("album_detail:empty_state");
            let _empty = empty_span.enter();
            let empty = empty_states::no_album_photos();
            empty.set_hexpand(true);
            empty.set_vexpand(true);
            obj.imp().content_box.get().append(&empty);
            tracing::debug!(
                target: crate::core::log_targets::ALBUMS,
                album_name = %album_name,
                album_path = %album_path,
                "album_detail_page: empty_state_built"
            );
        } else {
            let grid_span = tracing::info_span!("album_detail:grid_build");
            let _grid = grid_span.enter();
            let on_activate: Rc<dyn Fn(MediaId)> = {
                let weak = obj.downgrade();
                Rc::new(move |media_id| {
                    if let Some(this) = weak.upgrade() {
                        let Some(media_list) = this.imp().media_list.borrow().as_ref().cloned()
                        else {
                            return;
                        };
                        let Some(index) = index_for_media_id(&media_list, media_id) else {
                            return;
                        };
                        this.open_viewer(media_id, index);
                    }
                })
            };
            let on_background_changed: Rc<dyn Fn()> = Rc::new(|| {});
            let on_set_album_cover: Rc<dyn Fn(MediaId)> = {
                let weak = obj.downgrade();
                Rc::new(move |media_id| {
                    if let Some(this) = weak.upgrade() {
                        this.set_album_cover_from_media(media_id);
                    }
                })
            };
            let on_move_to_trash: Rc<dyn Fn(Vec<MediaId>)> = {
                let weak = obj.downgrade();
                Rc::new(move |ids| {
                    if let Some(this) = weak.upgrade() {
                        this.delete_to_trash_for_ids(ids);
                    }
                })
            };
            let grid = MediaGrid::new_for_album_with_context_menu(
                media_list,
                GroupBy::Day,
                loader,
                MediaGridCallbacks {
                    on_activate,
                    on_background_changed,
                    on_add_to_album: Rc::new(|_| {}),
                    on_move_to_trash,
                    on_set_favorite: Rc::new(|_, _| {}),
                    on_query_favorite_state: Rc::new(|_| FavoriteMenuState::default()),
                    on_set_album_cover: Some(on_set_album_cover),
                },
            );
            grid.set_context_menu_overlay(Some(&obj.imp().grid_overlay.get()));
            obj.imp().content_box.get().append(&grid);
            tracing::debug!(
                target: crate::core::log_targets::ALBUMS,
                album_name = %album_name,
                album_path = %album_path,
                "album_detail_page: grid_built"
            );
        }

        tracing::debug!(
            target: crate::core::log_targets::ALBUMS,
            album_name = %album_name,
            album_path = %album_path,
            item_count = initial_items,
            "album_detail_page: build_end"
        );

        // Wire the search button to open the search page.
        obj.imp()
            .search_btn
            .get()
            .set_tooltip_text(Some(&crate::core::i18n::tr("photos.search.tooltip")));
        {
            let weak = obj.downgrade();
            obj.imp().search_btn.get().connect_clicked(move |_| {
                if let Some(this) = weak.upgrade() {
                    this.open_search_page();
                }
            });
        }

        obj
    }

    pub fn set_db_actor(&self, db_actor: DbActorHandle) {
        *self.imp().db_actor.borrow_mut() = Some(db_actor);
    }

    pub fn set_nav_target(&self, nav: &adw::NavigationView) {
        *self.imp().nav_view.borrow_mut() = Some(nav.clone());
    }

    fn delete_to_trash_for_ids(&self, ids: Vec<MediaId>) {
        let Some(db_actor) = self.imp().db_actor.borrow().as_ref().cloned() else {
            tracing::warn!(
                target: crate::core::log_targets::ALBUMS,
                "TRASH_TRACE album_detail_delete_no_actor count={} ids={:?}",
                ids.len(),
                ids.iter().map(|id| id.get()).collect::<Vec<_>>()
            );
            return;
        };
        let Some(pool) = self.imp().pool.borrow().as_ref().cloned() else {
            tracing::warn!(
                target: crate::core::log_targets::ALBUMS,
                "TRASH_TRACE album_detail_delete_no_pool count={} ids={:?}",
                ids.len(),
                ids.iter().map(|id| id.get()).collect::<Vec<_>>()
            );
            return;
        };
        if ids.is_empty() {
            return;
        }
        tracing::info!(
            target: crate::core::log_targets::ALBUMS,
            "TRASH_TRACE album_detail_delete_requested count={} ids={:?}",
            ids.len(),
            ids.iter().map(|id| id.get()).collect::<Vec<_>>()
        );

        let weak = self.downgrade();
        let ids_for_worker = ids.clone();
        glib::spawn_future_local(async move {
            let prepared = db_actor
                .execute(DbCommand::MarkTrashed {
                    ids: ids_for_worker,
                })
                .await
                .ok();

            let Some(crate::core::DbCommandResult::MediaItems(items)) = prepared else {
                tracing::warn!(
                    target: crate::core::log_targets::ALBUMS,
                    "TRASH_TRACE album_detail_mark_failed"
                );
                return;
            };
            if let Some(this) = weak.upgrade() {
                crate::ui::trash_fallback::move_marked_items_with_fallback(
                    &this,
                    pool,
                    db_actor,
                    items,
                    move |moved_ids| {
                        if let Some(this) = weak.upgrade() {
                            this.remove_media_ids_from_lists(&moved_ids);
                        }
                    },
                );
            }
        });
    }

    fn remove_media_ids_from_lists(&self, ids: &[MediaId]) {
        if ids.is_empty() {
            return;
        }
        let raw_ids = ids.iter().map(|id| id.get()).collect::<HashSet<_>>();
        if let Some(list) = self.imp().media_list.borrow().as_ref() {
            remove_media_items_by_ids(list, &raw_ids);
        }
        if let Some(list) = self.imp().master_media_list.borrow().as_ref() {
            remove_media_items_by_ids(list, &raw_ids);
        }
    }

    pub(crate) fn open_search_page(&self) {
        let Some(nav) = self.imp().nav_view.borrow().as_ref().cloned() else {
            return;
        };
        let Some(pool) = self.imp().pool.borrow().as_ref().cloned() else {
            return;
        };
        let Some(loader) = self.imp().loader.borrow().as_ref().cloned() else {
            return;
        };
        let page = crate::ui::search_page::SearchPage::new(pool, loader);
        if let Some(db_actor) = self.imp().db_actor.borrow().as_ref().cloned() {
            page.set_db_actor(db_actor);
        }
        page.set_nav_target(&nav);
        nav.push(&page);
    }

    fn set_album_cover_from_media(&self, media_id: MediaId) {
        let Some(album) = self.imp().album.borrow().as_ref().cloned() else {
            return;
        };
        let Some(pool) = self.imp().pool.borrow().as_ref().cloned() else {
            return;
        };
        let Some(media_list) = self.imp().media_list.borrow().as_ref().cloned() else {
            return;
        };
        let Some(cover_uri) = media_uri_for_id(&media_list, media_id) else {
            return;
        };

        let folder_path = album.folder_path.clone();
        let nav = self.imp().nav_view.borrow().as_ref().cloned();
        glib::spawn_future_local(async move {
            let _ = gtk::gio::spawn_blocking(move || {
                albums::set_album_cover(&pool, &folder_path, &cover_uri)
            })
            .await;
            if let Some(nav) = nav {
                crate::ui::window::refresh_albums_sidebar(&nav);
            }
        });
    }

    /// The album's folder_path. The sidebar uses this to detect "already
    /// viewing this album" and avoid pushing a duplicate detail page.
    pub fn album_folder_path(&self) -> Option<std::path::PathBuf> {
        self.imp()
            .album
            .borrow()
            .as_ref()
            .map(|album| album.folder_path.clone())
    }

    fn open_viewer(&self, media_id: MediaId, global_index: u32) {
        if self.imp().viewer_open_pending.get() {
            tracing::debug!(
                target: crate::core::log_targets::ALBUMS,
                "AlbumDetailPage: ignoring duplicate viewer activation while push is pending"
            );
            return;
        }

        let media_list = match self.imp().media_list.borrow().as_ref() {
            Some(l) => l.clone(),
            None => return,
        };
        let nav = match self.imp().nav_view.borrow().as_ref() {
            Some(n) => n.clone(),
            None => return,
        };
        let self_page: adw::NavigationPage = self.clone().upcast();
        if nav
            .visible_page()
            .is_some_and(|visible| visible != self_page)
        {
            tracing::debug!(
                target: crate::core::log_targets::ALBUMS,
                "AlbumDetailPage: ignoring viewer activation because AlbumDetailPage is not visible"
            );
            return;
        }

        let query = self
            .imp()
            .album
            .borrow()
            .as_ref()
            .map(media_query_for_album)
            .unwrap_or(MediaQuery::LiveAll);
        let viewer = ViewerPage::new_for_query(query, media_id, media_list);
        if let Some(pool) = self.imp().pool.borrow().as_ref().cloned() {
            viewer.set_edit_target(&nav, pool.clone());
            let is_favorite_album = self
                .imp()
                .album
                .borrow()
                .as_ref()
                .is_some_and(|album| album.is_favorites_album());
            let this = self.downgrade();
            let nav_for_albums = nav.downgrade();
            viewer.connect_favorite_state_changed(move |_, _| {
                if is_favorite_album {
                    if let Some(this) = this.upgrade() {
                        this.refresh_media_list_from_repository();
                    }
                    if let Some(nav) = nav_for_albums.upgrade() {
                        crate::ui::window::refresh_albums_sidebar(&nav);
                    }
                }
            });
        }
        if let Some(db_actor) = self.imp().db_actor.borrow().as_ref().cloned() {
            viewer.set_db_actor(db_actor);
        }

        // Inject the shared thumbnail loader for the filmstrip.
        if let Some(loader) = self.imp().loader.borrow().as_ref().cloned() {
            viewer.set_thumbnail_loader(loader);
        }
        viewer.show_at(global_index);

        if let Some(master_list) = self.imp().master_media_list.borrow().as_ref().cloned() {
            viewer.connect_item_trashed(move |item_id| {
                remove_media_item_by_id(&master_list, item_id);
            });
        }

        let viewer_weak = viewer.downgrade();
        let nav_weak = nav.downgrade();
        viewer.connect_navigation(move |delta: NavDelta| {
            if delta == NAV_POP {
                if let Some(n) = nav_weak.upgrade() {
                    n.pop();
                }
                return;
            }
            if let Some(v) = viewer_weak.upgrade() {
                let cur = v.current_index();
                let next = (cur as i32 + delta).max(0) as u32;
                if let Some(list) = v.imp().media_list.borrow().as_ref() {
                    if next < list.n_items() {
                        v.show_at(next);
                    }
                }
            }
        });

        viewer.guard_initial_navigation_pop();
        self.imp().viewer_open_pending.set(true);
        let source_page = nav.visible_page();
        if let Some(page) = source_page.as_ref() {
            page.set_sensitive(false);
        }
        let weak = self.downgrade();
        glib::timeout_add_local_once(
            std::time::Duration::from_millis(VIEWER_OPEN_POP_GUARD_MS),
            move || {
                if let Some(this) = weak.upgrade() {
                    this.imp().viewer_open_pending.set(false);
                }
                if let Some(page) = source_page {
                    page.set_sensitive(true);
                }
            },
        );
        nav.push(&viewer);
    }

    #[tracing::instrument(name = "album_detail:refresh_media_list", skip(self))]
    pub(crate) fn refresh_media_list_from_repository(&self) {
        let Some(album) = self.imp().album.borrow().as_ref().cloned() else {
            return;
        };

        let Some(pool) = self.imp().pool.borrow().as_ref().cloned() else {
            return;
        };
        let Some(media_list) = self.imp().media_list.borrow().as_ref().cloned() else {
            return;
        };

        // Re-evaluate the per-album query against the current DB state and
        // splice it into the live store so the grid + open viewer track new
        // membership without pushing a new page.
        let repo = MediaRepository::new(pool);
        let query = media_query_for_album(&album);
        let total = repo
            .count(query.clone())
            .map(i64::from)
            .unwrap_or(album.photo_count);
        let limit = album_refresh_load_limit(total);
        let items = repo.items(query, 0, limit).unwrap_or_default();
        {
            let splice_span = tracing::info_span!("album_detail:splice");
            let _splice = splice_span.enter();
            if !apply_pure_insertions(&media_list, &items) {
                let additions: Vec<glib::BoxedAnyObject> =
                    items.into_iter().map(glib::BoxedAnyObject::new).collect();
                media_list.splice(0, media_list.n_items(), &additions);
            }
        }
        tracing::debug!(
            target: crate::core::log_targets::ALBUMS,
            album_name = %album.display_name(),
            album_path = %album.folder_path.display(),
            is_virtual = album.is_virtual,
            item_count = media_list.n_items(),
            "album_detail_page: refreshed_media_list"
        );
    }
}

fn apply_pure_insertions(list: &gtk::gio::ListStore, refreshed: &[MediaItem]) -> bool {
    let current = media_items_from_store(list);
    if refreshed.len() <= current.len() {
        return false;
    }

    let mut prefix = 0usize;
    while prefix < current.len()
        && prefix < refreshed.len()
        && same_media_identity(&current[prefix], &refreshed[prefix])
    {
        prefix += 1;
    }

    let mut suffix = 0usize;
    while suffix < current.len().saturating_sub(prefix)
        && same_media_identity(
            &current[current.len() - 1 - suffix],
            &refreshed[refreshed.len() - 1 - suffix],
        )
    {
        suffix += 1;
    }

    if prefix + suffix != current.len() {
        return false;
    }

    let insert_end = refreshed.len() - suffix;
    if insert_end <= prefix {
        return false;
    }
    let additions: Vec<glib::BoxedAnyObject> = refreshed[prefix..insert_end]
        .iter()
        .cloned()
        .map(glib::BoxedAnyObject::new)
        .collect();
    list.splice(prefix as u32, 0, &additions);
    true
}

fn media_items_from_store(list: &gtk::gio::ListStore) -> Vec<MediaItem> {
    let mut items = Vec::with_capacity(list.n_items() as usize);
    for idx in 0..list.n_items() {
        let Some(obj) = list.item(idx) else {
            continue;
        };
        let Ok(boxed) = obj.downcast::<glib::BoxedAnyObject>() else {
            continue;
        };
        items.push((*boxed.borrow::<MediaItem>()).clone());
    }
    items
}

fn same_media_identity(a: &MediaItem, b: &MediaItem) -> bool {
    a.id == b.id && a.uri == b.uri
}

#[cfg(test)]
#[tracing::instrument(name = "album_detail:filter_items", skip(album, master, pool))]
pub(crate) fn filtered_items_for_album_limited(
    album: &Album,
    master: &gtk::gio::ListStore,
    pool: &DbPool,
    limit: u32,
) -> Vec<crate::core::media::MediaItem> {
    let album_name = album.display_name();
    let album_path = album.folder_path.to_string_lossy().into_owned();
    if album.is_virtual {
        let result = MediaRepository::new(pool.clone())
            .page(media_query_for_album(album), 0, limit)
            .map(|page| page.items)
            .unwrap_or_default();
        tracing::debug!(
            target: crate::core::log_targets::ALBUMS,
            album_name = %album_name,
            album_path = %album_path,
            is_virtual = true,
            master_items = master.n_items(),
            result_items = result.len(),
            "album_filter: loaded_virtual_from_repository"
        );
        return result;
    }

    let mut items = Vec::new();
    for idx in 0..master.n_items() {
        let Some(obj) = master.item(idx) else {
            continue;
        };
        let Ok(boxed) = obj.downcast::<glib::BoxedAnyObject>() else {
            continue;
        };
        let item = (*boxed.borrow::<crate::core::media::MediaItem>()).clone();
        if item.folder_path == album.folder_path {
            items.push(item);
            if items.len() >= limit as usize {
                break;
            }
        }
    }
    tracing::debug!(
        target: crate::core::log_targets::ALBUMS,
        album_name = %album_name,
        album_path = %album_path,
        is_virtual = false,
        master_items = master.n_items(),
        result_items = items.len(),
        "album_filter: filtered_master_list"
    );
    items
}

fn album_refresh_load_limit(total: i64) -> u32 {
    let total = u32::try_from(total.max(0)).unwrap_or(u32::MAX);
    let ui_cap =
        u32::try_from(crate::core::runtime_config::ui_media_list_cap()).unwrap_or(u32::MAX);
    total.min(ui_cap)
}

pub(crate) fn media_query_for_album(album: &Album) -> MediaQuery {
    if album.is_favorites_album() {
        MediaQuery::Favorites
    } else if album.is_images_album() {
        MediaQuery::Images
    } else if album.is_videos_album() {
        MediaQuery::Videos
    } else if album.is_motion_photos_album() {
        MediaQuery::MotionPhotos
    } else if album.is_animated_album() {
        MediaQuery::Attribute(crate::core::media::MEDIA_ATTRIBUTE_ANIMATED.into())
    } else if album.is_hdr_album() {
        MediaQuery::Attribute(crate::core::media::MEDIA_ATTRIBUTE_HDR.into())
    } else {
        MediaQuery::AlbumFolder(album.folder_path.clone())
    }
}

fn remove_media_item_by_id(list: &gtk::gio::ListStore, item_id: i64) -> bool {
    for idx in 0..list.n_items() {
        let Some(obj) = list.item(idx) else {
            continue;
        };
        let Ok(boxed) = obj.downcast::<glib::BoxedAnyObject>() else {
            continue;
        };
        if boxed.borrow::<crate::core::media::MediaItem>().id == item_id {
            list.remove(idx);
            return true;
        }
    }
    false
}

fn remove_media_items_by_ids(list: &gtk::gio::ListStore, ids: &HashSet<i64>) {
    let mut idx = list.n_items();
    while idx > 0 {
        idx -= 1;
        let Some(obj) = list.item(idx) else {
            continue;
        };
        let Ok(boxed) = obj.downcast::<glib::BoxedAnyObject>() else {
            continue;
        };
        if ids.contains(&boxed.borrow::<MediaItem>().id) {
            list.remove(idx);
        }
    }
}

fn index_for_media_id(list: &gtk::gio::ListStore, media_id: MediaId) -> Option<u32> {
    for index in 0..list.n_items() {
        let Some(obj) = list.item(index) else {
            continue;
        };
        let Ok(boxed) = obj.downcast::<glib::BoxedAnyObject>() else {
            continue;
        };
        if boxed.borrow::<MediaItem>().id == media_id.get() {
            return Some(index);
        }
    }
    None
}

fn media_uri_for_id(list: &gtk::gio::ListStore, media_id: MediaId) -> Option<String> {
    for i in 0..list.n_items() {
        let Some(obj) = list.item(i) else {
            continue;
        };
        let Ok(boxed) = obj.downcast::<glib::BoxedAnyObject>() else {
            continue;
        };
        let item = boxed.borrow::<MediaItem>();
        if item.id == media_id.get() {
            return Some(item.uri.clone());
        }
    }
    None
}

impl Default for AlbumDetailPage {
    fn default() -> Self {
        glib::Object::builder().build()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use std::path::PathBuf;

    fn sample_item(id: i64) -> crate::core::media::MediaItem {
        let dt = Utc.with_ymd_and_hms(2026, 6, 23, 12, 0, 0).unwrap();
        crate::core::media::MediaItem {
            id,
            uri: format!("file:///tmp/{id}.jpg"),
            path: PathBuf::from(format!("/tmp/{id}.jpg")),
            folder_path: PathBuf::from("/tmp"),
            mime_type: "image/jpeg".into(),
            media_subkind: "standard".into(),
            media_attributes: "{}".into(),
            width: Some(100),
            height: Some(100),
            video_duration_secs: None,
            taken_at: Some(dt),
            file_mtime: dt,
            file_size: 100,
            blake3_hash: format!("hash-{id}"),
            is_favorite: false,
            trashed_at: None,
        }
    }

    fn new_item(id: i64, mime_type: &str) -> crate::core::media::NewMediaItem {
        let dt = Utc.with_ymd_and_hms(2026, 6, 23, 12, 0, 0).unwrap();
        let ext = if mime_type.starts_with("video/") {
            "mp4"
        } else {
            "jpg"
        };
        let path = PathBuf::from(format!("/tmp/{id}.{ext}"));
        crate::core::media::NewMediaItem {
            uri: format!("file:///tmp/{id}.{ext}"),
            path,
            folder_path: PathBuf::from("/tmp"),
            mime_type: mime_type.into(),
            media_subkind: "standard".into(),
            media_attributes: "{}".into(),
            width: Some(100),
            height: Some(100),
            video_duration_secs: None,
            taken_at: Some(dt),
            file_mtime: dt,
            file_size: 100,
            blake3_hash: format!("hash-new-{id}"),
        }
    }

    #[gtk::test]
    fn remove_media_item_by_id_updates_shared_master_list() {
        let _ = gtk::init();
        let list = gtk::gio::ListStore::new::<glib::BoxedAnyObject>();
        list.append(&glib::BoxedAnyObject::new(sample_item(1)));
        list.append(&glib::BoxedAnyObject::new(sample_item(2)));

        assert!(remove_media_item_by_id(&list, 1));
        assert_eq!(list.n_items(), 1);
        assert!(!remove_media_item_by_id(&list, 3));
    }

    #[gtk::test]
    fn video_virtual_album_uses_database_when_master_window_has_no_videos() {
        let _ = gtk::init();
        let tmp = tempfile::tempdir().unwrap();
        let pool = crate::core::db::init_pool(&tmp.path().join("videos.db")).unwrap();
        crate::core::db::upsert_media_items_batch(
            &pool,
            &[new_item(1, "image/jpeg"), new_item(2, "video/mp4")],
        )
        .unwrap();
        let album = crate::core::albums::Album {
            folder_path: PathBuf::from(crate::core::albums::VIDEOS_ALBUM_PATH),
            name: "Videos".into(),
            cover_uri: None,
            photo_count: 1,
            last_modified: Utc.with_ymd_and_hms(2026, 6, 23, 12, 0, 0).unwrap(),
            is_virtual: true,
        };
        let master = gtk::gio::ListStore::new::<glib::BoxedAnyObject>();
        master.append(&glib::BoxedAnyObject::new(sample_item(1)));

        let items = filtered_items_for_album_limited(&album, &master, &pool, u32::MAX);

        assert_eq!(items.len(), 1);
        assert!(items[0].is_video());
    }

    #[gtk::test]
    fn virtual_album_filter_can_limit_database_membership() {
        let _ = gtk::init();
        let tmp = tempfile::tempdir().unwrap();
        let pool = crate::core::db::init_pool(&tmp.path().join("limited-videos.db")).unwrap();
        crate::core::db::upsert_media_items_batch(
            &pool,
            &[
                new_item(1, "video/mp4"),
                new_item(2, "video/mp4"),
                new_item(3, "video/mp4"),
            ],
        )
        .unwrap();
        let album = crate::core::albums::Album {
            folder_path: PathBuf::from(crate::core::albums::VIDEOS_ALBUM_PATH),
            name: "Videos".into(),
            cover_uri: None,
            photo_count: 3,
            last_modified: Utc.with_ymd_and_hms(2026, 6, 23, 12, 0, 0).unwrap(),
            is_virtual: true,
        };
        let master = gtk::gio::ListStore::new::<glib::BoxedAnyObject>();

        let items = filtered_items_for_album_limited(&album, &master, &pool, 2);

        assert_eq!(items.len(), 2);
    }

    #[gtk::test]
    fn visible_real_album_refresh_loads_new_database_items() {
        let _ = gtk::init();
        let tmp = tempfile::tempdir().unwrap();
        let pool = crate::core::db::init_pool(&tmp.path().join("real-album-refresh.db")).unwrap();
        let first = new_item(1, "image/jpeg");
        crate::core::db::insert_media_item(&pool, &first).unwrap();
        let initial =
            crate::core::db::list_media_by_folder_page(&pool, &first.folder_path, 0, 10).unwrap();
        assert_eq!(initial.len(), 1);

        let album = crate::core::albums::Album {
            folder_path: first.folder_path.clone(),
            name: "tmp".into(),
            cover_uri: None,
            photo_count: 1,
            last_modified: Utc.with_ymd_and_hms(2026, 6, 23, 12, 0, 0).unwrap(),
            is_virtual: false,
        };
        let album_store = gtk::gio::ListStore::new::<glib::BoxedAnyObject>();
        album_store.append(&glib::BoxedAnyObject::new(initial[0].clone()));
        let master = gtk::gio::ListStore::new::<glib::BoxedAnyObject>();
        master.append(&glib::BoxedAnyObject::new(initial[0].clone()));
        let loader = Arc::new(crate::core::thumbnails::ThumbnailLoader::new(
            pool.clone(),
            tmp.path().join("thumbs"),
        ));
        let page = AlbumDetailPage::new(album, album_store.clone(), master, pool.clone(), loader);

        crate::core::db::insert_media_item(&pool, &new_item(2, "image/jpeg")).unwrap();
        page.refresh_media_list_from_repository();

        assert_eq!(
            album_store.n_items(),
            2,
            "refreshing a visible real album should use the database, not the stale Photos window"
        );
    }

    #[gtk::test]
    fn visible_real_album_refresh_emits_single_addition_change() {
        let _ = gtk::init();
        let tmp = tempfile::tempdir().unwrap();
        let pool =
            crate::core::db::init_pool(&tmp.path().join("real-album-refresh-single.db")).unwrap();
        let first = new_item(1, "image/jpeg");
        crate::core::db::insert_media_item(&pool, &first).unwrap();
        let initial =
            crate::core::db::list_media_by_folder_page(&pool, &first.folder_path, 0, 10).unwrap();
        let album = crate::core::albums::Album {
            folder_path: first.folder_path.clone(),
            name: "tmp".into(),
            cover_uri: None,
            photo_count: 1,
            last_modified: Utc.with_ymd_and_hms(2026, 6, 23, 12, 0, 0).unwrap(),
            is_virtual: false,
        };
        let album_store = gtk::gio::ListStore::new::<glib::BoxedAnyObject>();
        album_store.append(&glib::BoxedAnyObject::new(initial[0].clone()));
        let master = gtk::gio::ListStore::new::<glib::BoxedAnyObject>();
        master.append(&glib::BoxedAnyObject::new(initial[0].clone()));
        let loader = Arc::new(crate::core::thumbnails::ThumbnailLoader::new(
            pool.clone(),
            tmp.path().join("thumbs"),
        ));
        let page = AlbumDetailPage::new(album, album_store.clone(), master, pool.clone(), loader);
        let changes = Rc::new(RefCell::new(Vec::<(u32, u32, u32)>::new()));
        let changes_for_signal = changes.clone();
        album_store.connect_items_changed(move |_, position, removed, added| {
            changes_for_signal
                .borrow_mut()
                .push((position, removed, added));
        });

        crate::core::db::insert_media_item(&pool, &new_item(2, "image/jpeg")).unwrap();
        page.refresh_media_list_from_repository();

        assert_eq!(
            *changes.borrow(),
            vec![(0, 0, 1)],
            "album refresh should emit one pure-addition change so MediaGrid can use its insertion path"
        );
    }
}
