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

use crate::core::albums::Album;
use crate::core::db::DbPool;
use crate::core::section_model::GroupBy;
use crate::core::thumbnails::ThumbnailLoader;
use crate::ui::empty_states;
use crate::ui::media_grid::{FavoriteMenuState, MediaGrid};
use crate::ui::viewer_page::{NavDelta, ViewerPage, NAV_POP};
use std::cell::RefCell;
use std::rc::Rc;

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(file = "../../data/ui/album-detail-page.ui")]
    pub struct AlbumDetailPage {
        pub media_list: RefCell<Option<gtk::gio::ListStore>>,
        pub master_media_list: RefCell<Option<gtk::gio::ListStore>>,
        pub pool: RefCell<Option<DbPool>>,
        pub album: RefCell<Option<Album>>,
        pub nav_view: RefCell<Option<adw::NavigationView>>,
        pub loader: RefCell<Option<Arc<ThumbnailLoader>>>,
        #[template_child]
        pub header_bar: TemplateChild<adw::HeaderBar>,
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
    pub fn new(
        album: Album,
        media_list: gtk::gio::ListStore,
        master_media_list: gtk::gio::ListStore,
        pool: DbPool,
        loader: Arc<ThumbnailLoader>,
    ) -> Self {
        let obj: Self = glib::Object::builder().build();
        obj.set_title(&album.display_name());
        *obj.imp().media_list.borrow_mut() = Some(media_list.clone());
        *obj.imp().master_media_list.borrow_mut() = Some(master_media_list);
        *obj.imp().album.borrow_mut() = Some(album);
        *obj.imp().pool.borrow_mut() = Some(pool);
        *obj.imp().loader.borrow_mut() = Some(loader.clone());

        if media_list.n_items() == 0 {
            let empty = empty_states::no_album_photos();
            empty.set_hexpand(true);
            empty.set_vexpand(true);
            obj.imp().content_box.get().append(&empty);
        } else {
            let on_activate: Rc<dyn Fn(u32)> = {
                let weak = obj.downgrade();
                Rc::new(move |global_index| {
                    if let Some(this) = weak.upgrade() {
                        this.open_viewer(global_index);
                    }
                })
            };
            let on_background_changed: Rc<dyn Fn()> = Rc::new(|| {});
            let grid = MediaGrid::new(
                media_list,
                GroupBy::Day,
                loader,
                on_activate,
                on_background_changed,
                Rc::new(|_| {}),
                Rc::new(|_| {}),
                Rc::new(|_, _| {}),
                Rc::new(|_| FavoriteMenuState::default()),
                false,
            );
            obj.imp().content_box.get().append(&grid);
        }

        obj
    }

    pub fn set_nav_target(&self, nav: &adw::NavigationView) {
        *self.imp().nav_view.borrow_mut() = Some(nav.clone());
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

    fn open_viewer(&self, global_index: u32) {
        let media_list = match self.imp().media_list.borrow().as_ref() {
            Some(l) => l.clone(),
            None => return,
        };
        let nav = match self.imp().nav_view.borrow().as_ref() {
            Some(n) => n.clone(),
            None => return,
        };

        let viewer = ViewerPage::new(media_list, global_index);
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
                        this.refresh_virtual_album_media_list();
                    }
                    if let Some(nav) = nav_for_albums.upgrade() {
                        crate::ui::window::refresh_albums_sidebar(&nav);
                    }
                }
            });
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

        nav.push(&viewer);
    }

    fn refresh_virtual_album_media_list(&self) {
        let Some(album) = self.imp().album.borrow().as_ref().cloned() else {
            return;
        };
        if !album.is_virtual {
            return;
        }

        let Some(pool) = self.imp().pool.borrow().as_ref().cloned() else {
            return;
        };
        let Some(master_list) = self.imp().master_media_list.borrow().as_ref().cloned() else {
            return;
        };
        let Some(media_list) = self.imp().media_list.borrow().as_ref().cloned() else {
            return;
        };

        // Re-evaluate the per-album predicate (shared with the sidebar) against
        // the current DB/favorites state and splice it into the live store so
        // the grid + open viewer track the new membership without a page rebuild.
        let items = filtered_items_for_album(&album, &master_list, &pool);
        while media_list.n_items() > 0 {
            media_list.remove(media_list.n_items() - 1);
        }
        for item in items {
            media_list.append(&glib::BoxedAnyObject::new(item));
        }
        tracing::debug!(
            target: crate::core::log_targets::ALBUMS,
            "AlbumDetailPage: refreshed virtual album media list, n_items={}",
            media_list.n_items()
        );
    }
}

/// Build the per-album filtered media items from the shared master list.
///
/// Mirrors the album semantics established by `albums::list_with_favorites`:
/// favorites album → the favorites id set; images/videos albums → `media_kind`;
/// folder albums → equal `folder_path`. The sidebar builds an `AlbumDetailPage`
/// from this, and the favorites album refreshes its already-attached media
/// list on favorite toggles via [`AlbumDetailPage::refresh_virtual_album_media_list`].
pub(crate) fn filtered_items_for_album(
    album: &Album,
    master: &gtk::gio::ListStore,
    pool: &DbPool,
) -> Vec<crate::core::media::MediaItem> {
    let favorite_ids: HashSet<i64> = if album.is_favorites_album() {
        crate::core::albums::favorite_media_ids(pool)
            .unwrap_or_default()
            .into_iter()
            .collect()
    } else {
        HashSet::new()
    };
    let mut items = Vec::new();
    for idx in 0..master.n_items() {
        let Some(obj) = master.item(idx) else {
            continue;
        };
        let Ok(boxed) = obj.downcast::<glib::BoxedAnyObject>() else {
            continue;
        };
        let item = (*boxed.borrow::<crate::core::media::MediaItem>()).clone();
        let should_include = if album.is_favorites_album() {
            favorite_ids.contains(&item.id)
        } else if album.is_images_album() {
            item.is_image()
        } else if album.is_videos_album() {
            item.is_video()
        } else {
            item.folder_path == album.folder_path
        };
        if should_include {
            items.push(item);
        }
    }
    items
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
            width: Some(100),
            height: Some(100),
            taken_at: Some(dt),
            file_mtime: dt,
            file_size: 100,
            blake3_hash: format!("hash-{id}"),
            trashed_at: None,
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
}
