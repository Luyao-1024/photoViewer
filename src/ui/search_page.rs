use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use gtk4 as gtk;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::{NavigationPageExt, WidgetExt};
use libadwaita::subclass::prelude::*;

use crate::core::db::DbPool;
use crate::core::i18n::tr;
use crate::core::identity::MediaId;
use crate::core::media::MediaItem;
use crate::core::repository::{MediaQuery, MediaRepository};
use crate::core::runtime_config;
use crate::core::section_model::GroupBy;
use crate::core::thumbnails::ThumbnailLoader;
use crate::ui::media_grid::{FavoriteMenuState, MediaGrid, MediaGridCallbacks};
use crate::ui::viewer_page::{NavDelta, ViewerPage, NAV_POP};

const SEARCH_PREVIEW_FALLBACK_COLUMNS: usize = 8;
const SEARCH_PREVIEW_MIN_ROWS: usize = 2;
const SEARCH_YEAR_TILE_SIZE: i32 = 90;
const SEARCH_YEAR_TILE_GAP: i32 = 8;
const SEARCH_SECTION_HEADER_HEIGHT: i32 = 40;

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(file = "../../data/ui/search-page.ui")]
    pub struct SearchPage {
        pub pool: RefCell<Option<DbPool>>,
        pub loader: RefCell<Option<Arc<ThumbnailLoader>>>,
        pub nav_view: RefCell<Option<adw::NavigationView>>,
        pub image_list: RefCell<Option<gtk::gio::ListStore>>,
        pub video_list: RefCell<Option<gtk::gio::ListStore>>,
        pub image_full_results: RefCell<Vec<MediaItem>>,
        pub video_full_results: RefCell<Vec<MediaItem>>,
        pub image_grid: RefCell<Option<MediaGrid>>,
        pub video_grid: RefCell<Option<MediaGrid>>,
        pub image_more_tile: RefCell<Option<gtk::Button>>,
        pub video_more_tile: RefCell<Option<gtk::Button>>,
        pub preview_capacity: Cell<usize>,
        pub search_generation: Cell<u64>,
        #[template_child]
        pub header_bar: TemplateChild<adw::HeaderBar>,
        #[template_child]
        pub search_entry: TemplateChild<gtk::SearchEntry>,
        #[template_child]
        pub content_box: TemplateChild<gtk::Box>,
        #[template_child]
        pub image_results_box: TemplateChild<gtk::Box>,
        #[template_child]
        pub video_results_box: TemplateChild<gtk::Box>,
    }

    #[gtk::glib::object_subclass]
    impl ObjectSubclass for SearchPage {
        const NAME: &'static str = "SearchPage";
        type Type = super::SearchPage;
        type ParentType = adw::NavigationPage;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &gtk::glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for SearchPage {}
    impl WidgetImpl for SearchPage {}
    impl NavigationPageImpl for SearchPage {}
}

gtk::glib::wrapper! {
    pub struct SearchPage(ObjectSubclass<imp::SearchPage>)
        @extends adw::NavigationPage, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable;
}

impl SearchPage {
    pub fn new(pool: DbPool, loader: Arc<ThumbnailLoader>) -> Self {
        let obj: Self = glib::Object::builder().build();
        obj.set_title(&tr("search.title"));
        obj.imp()
            .search_entry
            .get()
            .set_placeholder_text(Some(&tr("search.placeholder")));

        let image_list = gtk::gio::ListStore::new::<glib::BoxedAnyObject>();
        let video_list = gtk::gio::ListStore::new::<glib::BoxedAnyObject>();

        *obj.imp().pool.borrow_mut() = Some(pool);
        *obj.imp().loader.borrow_mut() = Some(loader.clone());
        *obj.imp().image_list.borrow_mut() = Some(image_list.clone());
        *obj.imp().video_list.borrow_mut() = Some(video_list.clone());

        let image_grid = obj.build_result_section(
            &obj.imp().image_results_box.get(),
            &tr("search.images"),
            image_list,
            loader.clone(),
            "image",
        );
        let video_grid = obj.build_result_section(
            &obj.imp().video_results_box.get(),
            &tr("search.videos"),
            video_list,
            loader,
            "video",
        );
        *obj.imp().image_grid.borrow_mut() = Some(image_grid);
        *obj.imp().video_grid.borrow_mut() = Some(video_grid);
        obj.imp().image_results_box.get().set_visible(false);
        obj.imp().video_results_box.get().set_visible(false);
        obj.imp()
            .preview_capacity
            .set(Self::fallback_preview_capacity());

        let weak = obj.downgrade();
        obj.imp()
            .search_entry
            .get()
            .connect_search_changed(move |entry| {
                if let Some(this) = weak.upgrade() {
                    this.run_search(entry.text().as_str());
                }
            });

        let entry = obj.imp().search_entry.get();
        glib::idle_add_local_once(move || {
            entry.grab_focus();
        });

        let weak = obj.downgrade();
        obj.add_tick_callback(move |_, _| {
            if let Some(this) = weak.upgrade() {
                this.refresh_preview_capacity_from_layout();
                glib::ControlFlow::Continue
            } else {
                glib::ControlFlow::Break
            }
        });

        obj
    }

    pub(crate) fn focus_search_entry(&self) {
        self.imp().search_entry.get().grab_focus();
    }

    pub fn set_nav_target(&self, nav: &adw::NavigationView) {
        *self.imp().nav_view.borrow_mut() = Some(nav.clone());
    }

    pub fn preview_item_limit() -> usize {
        Self::fallback_preview_capacity()
    }

    fn fallback_preview_capacity() -> usize {
        SEARCH_PREVIEW_FALLBACK_COLUMNS * SEARCH_PREVIEW_MIN_ROWS
    }

    pub fn preview_capacity_for_width(width: i32) -> usize {
        Self::preview_capacity_for_area(width, 0, 1)
    }

    pub fn preview_capacity_for_area(width: i32, height: i32, visible_sections: usize) -> usize {
        let available = width.max(0);
        if available <= 0 {
            return Self::fallback_preview_capacity();
        }
        let tile_step = (SEARCH_YEAR_TILE_SIZE + SEARCH_YEAR_TILE_GAP).max(1);
        let columns = (available / tile_step).max(1) as usize;
        let available_height = height.max(0);
        if available_height <= 0 {
            return columns * SEARCH_PREVIEW_MIN_ROWS;
        }
        let sections = visible_sections.max(1) as i32;
        let section_height =
            (available_height - sections * SEARCH_SECTION_HEADER_HEIGHT).max(tile_step);
        let rows = (section_height / sections / tile_step).max(SEARCH_PREVIEW_MIN_ROWS as i32);
        columns * rows as usize
    }

    fn build_result_section(
        &self,
        section: &gtk::Box,
        title: &str,
        media_list: gtk::gio::ListStore,
        loader: Arc<ThumbnailLoader>,
        media_kind: &'static str,
    ) -> MediaGrid {
        let header = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(8)
            .hexpand(true)
            .build();
        let label = gtk::Label::builder()
            .label(title)
            .halign(gtk::Align::Start)
            .hexpand(true)
            .css_classes(["search-results-title"])
            .build();
        header.append(&label);
        section.append(&header);

        let on_activate: Rc<dyn Fn(MediaId)> = {
            let weak = self.downgrade();
            let media_list = media_list.clone();
            Rc::new(move |media_id| {
                if let Some(this) = weak.upgrade() {
                    this.open_viewer(media_id, media_kind, media_list.clone());
                }
            })
        };
        let grid = MediaGrid::new_for_album(
            media_list,
            GroupBy::Year,
            loader,
            MediaGridCallbacks {
                on_activate,
                on_background_changed: Rc::new(|| {}),
                on_add_to_album: Rc::new(|_| {}),
                on_move_to_trash: Rc::new(|_| {}),
                on_set_favorite: Rc::new(|_, _| {}),
                on_query_favorite_state: Rc::new(|_| FavoriteMenuState::default()),
            },
        );
        grid.set_flat_sections(true);
        grid.set_content_sized_scroll(560);
        section.append(&grid);

        // "Show more" tile — created here, but appended into the grid's
        // FlowBox after each replace_results (because MediaGrid rebuilds its
        // FlowBox when the media list changes).
        let more_btn = gtk::Button::builder()
            .label("更多...")
            .css_classes(["search-more-tile"])
            .build();
        let weak = self.downgrade();
        more_btn.connect_clicked(move |_| {
            if let Some(this) = weak.upgrade() {
                this.open_more_results(media_kind);
            }
        });
        more_btn.set_visible(false);

        match media_kind {
            "image" => *self.imp().image_more_tile.borrow_mut() = Some(more_btn.clone()),
            "video" => *self.imp().video_more_tile.borrow_mut() = Some(more_btn),
            _ => {}
        }

        grid
    }

    fn run_search(&self, raw_text: &str) {
        let term = raw_text.trim().to_string();
        let generation = self.imp().search_generation.get().saturating_add(1);
        self.imp().search_generation.set(generation);
        if term.is_empty() {
            self.replace_results(Vec::new(), Vec::new());
            return;
        }
        let Some(pool) = self.imp().pool.borrow().as_ref().cloned() else {
            return;
        };
        let limit = runtime_config::ui_media_list_cap().min(u32::MAX as usize) as u32;
        let weak = self.downgrade();
        glib::spawn_future_local(async move {
            let result = gtk::gio::spawn_blocking(move || {
                let repo = MediaRepository::new(pool);
                let images = repo.items(
                    MediaQuery::SearchKind {
                        term: term.clone(),
                        media_kind: "image".into(),
                    },
                    0,
                    limit,
                )?;
                let videos = repo.items(
                    MediaQuery::SearchKind {
                        term,
                        media_kind: "video".into(),
                    },
                    0,
                    limit,
                )?;
                crate::core::error::Result::<(Vec<MediaItem>, Vec<MediaItem>)>::Ok((images, videos))
            })
            .await;

            let (images, videos) = match result {
                Ok(Ok(result)) => result,
                Ok(Err(err)) => {
                    tracing::warn!("search page query failed: {err}");
                    return;
                }
                Err(err) => {
                    tracing::warn!("search page query join failed: {err:?}");
                    return;
                }
            };

            let Some(this) = weak.upgrade() else {
                return;
            };
            if this.imp().search_generation.get() != generation {
                return;
            }
            this.replace_results(images, videos);
        });
    }

    fn replace_results(&self, images: Vec<MediaItem>, videos: Vec<MediaItem>) {
        self.refresh_preview_capacity_from_layout();
        let preview_capacity = self.imp().preview_capacity.get();
        let has_images = !images.is_empty();
        let has_videos = !videos.is_empty();
        let image_has_more = images.len() > preview_capacity;
        let video_has_more = videos.len() > preview_capacity;
        // Reserve one slot for the "show more" tile when results are truncated.
        let image_preview = if image_has_more {
            preview_capacity.saturating_sub(1)
        } else {
            preview_capacity
        };
        let video_preview = if video_has_more {
            preview_capacity.saturating_sub(1)
        } else {
            preview_capacity
        };
        *self.imp().image_full_results.borrow_mut() = images.clone();
        *self.imp().video_full_results.borrow_mut() = videos.clone();
        self.imp().image_results_box.get().set_visible(has_images);
        self.imp().video_results_box.get().set_visible(has_videos);
        if let Some(btn) = self.imp().image_more_tile.borrow().as_ref() {
            btn.set_visible(image_has_more);
        }
        if let Some(btn) = self.imp().video_more_tile.borrow().as_ref() {
            btn.set_visible(video_has_more);
        }
        if let Some(list) = self.imp().image_list.borrow().as_ref() {
            replace_store_items(list, preview_items(&images, image_preview));
        }
        if let Some(list) = self.imp().video_list.borrow().as_ref() {
            replace_store_items(list, preview_items(&videos, video_preview));
        }
        // Re-append tiles after list update (grid rebuilds its FlowBox).
        self.reattach_more_tiles();
    }

    /// Re-append the "show more" tiles into their grids' FlowBoxes.
    /// MediaGrid rebuilds its FlowBox content whenever the media list changes,
    /// so the tiles must be re-appended after each list update.
    fn reattach_more_tiles(&self) {
        if let (Some(btn), Some(grid)) = (
            self.imp().image_more_tile.borrow().as_ref(),
            self.imp().image_grid.borrow().as_ref(),
        ) {
            grid.append_extra_child(btn.upcast_ref());
        }
        if let (Some(btn), Some(grid)) = (
            self.imp().video_more_tile.borrow().as_ref(),
            self.imp().video_grid.borrow().as_ref(),
        ) {
            grid.append_extra_child(btn.upcast_ref());
        }
    }

    fn refresh_preview_capacity_from_layout(&self) {
        let width = self.imp().image_results_box.get().allocated_width();
        let height = self.imp().content_box.get().allocated_height();
        let visible_sections = [
            !self.imp().image_full_results.borrow().is_empty(),
            !self.imp().video_full_results.borrow().is_empty(),
        ]
        .into_iter()
        .filter(|visible| *visible)
        .count()
        .max(1);
        let capacity = Self::preview_capacity_for_area(width, height, visible_sections);
        if self.imp().preview_capacity.replace(capacity) == capacity {
            return;
        }
        self.rebuild_previews_from_full_results();
    }

    fn rebuild_previews_from_full_results(&self) {
        let preview_capacity = self.imp().preview_capacity.get();
        let images = self.imp().image_full_results.borrow().clone();
        let videos = self.imp().video_full_results.borrow().clone();
        let image_has_more = images.len() > preview_capacity;
        let video_has_more = videos.len() > preview_capacity;
        let image_preview = if image_has_more {
            preview_capacity.saturating_sub(1)
        } else {
            preview_capacity
        };
        let video_preview = if video_has_more {
            preview_capacity.saturating_sub(1)
        } else {
            preview_capacity
        };
        if let Some(btn) = self.imp().image_more_tile.borrow().as_ref() {
            btn.set_visible(image_has_more);
        }
        if let Some(btn) = self.imp().video_more_tile.borrow().as_ref() {
            btn.set_visible(video_has_more);
        }
        if let Some(list) = self.imp().image_list.borrow().as_ref() {
            replace_store_items(list, preview_items(&images, image_preview));
        }
        if let Some(list) = self.imp().video_list.borrow().as_ref() {
            replace_store_items(list, preview_items(&videos, video_preview));
        }
        // Re-append tiles after list update (grid rebuilds its FlowBox).
        self.reattach_more_tiles();
    }

    fn open_more_results(&self, media_kind: &'static str) {
        let Some(nav) = self.imp().nav_view.borrow().as_ref().cloned() else {
            return;
        };
        let Some(loader) = self.imp().loader.borrow().as_ref().cloned() else {
            return;
        };
        let (title, items) = match media_kind {
            "image" => (
                tr("search.images"),
                self.imp().image_full_results.borrow().clone(),
            ),
            "video" => (
                tr("search.videos"),
                self.imp().video_full_results.borrow().clone(),
            ),
            _ => return,
        };
        if items.is_empty() {
            return;
        }

        let media_list = gtk::gio::ListStore::new::<glib::BoxedAnyObject>();
        replace_store_items(&media_list, items);

        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .build();
        let header = adw::HeaderBar::builder()
            .show_end_title_buttons(true)
            .css_classes(["glass-header"])
            .build();
        root.append(&header);

        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .margin_start(16)
            .margin_end(16)
            .margin_bottom(16)
            .vexpand(true)
            .hexpand(true)
            .build();
        let on_activate: Rc<dyn Fn(MediaId)> = {
            let weak = self.downgrade();
            let media_list = media_list.clone();
            Rc::new(move |media_id| {
                if let Some(this) = weak.upgrade() {
                    this.open_viewer(media_id, media_kind, media_list.clone());
                }
            })
        };
        let grid = MediaGrid::new_for_album(
            media_list,
            GroupBy::Year,
            loader,
            MediaGridCallbacks {
                on_activate,
                on_background_changed: Rc::new(|| {}),
                on_add_to_album: Rc::new(|_| {}),
                on_move_to_trash: Rc::new(|_| {}),
                on_set_favorite: Rc::new(|_, _| {}),
                on_query_favorite_state: Rc::new(|_| FavoriteMenuState::default()),
            },
        );
        content.append(&grid);
        root.append(&content);

        let page = adw::NavigationPage::builder()
            .title(title)
            .child(&root)
            .build();
        nav.push(&page);
    }

    fn open_viewer(
        &self,
        media_id: MediaId,
        media_kind: &'static str,
        media_list: gtk::gio::ListStore,
    ) {
        let Some(nav) = self.imp().nav_view.borrow().as_ref().cloned() else {
            return;
        };
        let Some(index) = index_for_media_id(&media_list, media_id) else {
            return;
        };
        let term = self.imp().search_entry.get().text().trim().to_string();
        let viewer = ViewerPage::new_for_query(
            MediaQuery::SearchKind {
                term,
                media_kind: media_kind.into(),
            },
            media_id,
            media_list,
        );
        if let Some(pool) = self.imp().pool.borrow().as_ref().cloned() {
            viewer.set_edit_target(&nav, pool);
        }
        if let Some(loader) = self.imp().loader.borrow().as_ref().cloned() {
            viewer.set_thumbnail_loader(loader);
        }
        viewer.show_at(index);

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
}

fn replace_store_items(store: &gtk::gio::ListStore, items: Vec<MediaItem>) {
    let additions: Vec<glib::BoxedAnyObject> =
        items.into_iter().map(glib::BoxedAnyObject::new).collect();
    store.splice(0, store.n_items(), &additions);
}

fn preview_items(items: &[MediaItem], limit: usize) -> Vec<MediaItem> {
    items.iter().take(limit).cloned().collect()
}

fn index_for_media_id(media_list: &gtk::gio::ListStore, media_id: MediaId) -> Option<u32> {
    for idx in 0..media_list.n_items() {
        let Some(obj) = media_list.item(idx) else {
            continue;
        };
        let Ok(boxed) = obj.downcast::<glib::BoxedAnyObject>() else {
            continue;
        };
        if boxed.borrow::<MediaItem>().id == media_id.get() {
            return Some(idx);
        }
    }
    None
}
