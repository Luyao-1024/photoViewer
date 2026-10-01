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
use crate::core::db_actor::{DbActorHandle, DbCommand};
use crate::core::identity::MediaId;
use crate::core::media::MediaItem;
use crate::core::repository::{MediaQuery, MediaRepository};
use crate::core::section_model::GroupBy;
use crate::core::thumbnails::ThumbnailLoader;
use crate::ui::album_picker;
use crate::ui::empty_states;
use crate::ui::keyboard::{KeyboardAction, KeyboardResult};
use crate::ui::media_grid::{FavoriteMenuState, MediaGridCallbacks};
use crate::ui::viewer_page::{NavDelta, ViewerPage, NAV_POP, VIEWER_OPEN_POP_GUARD_MS};
use crate::ui::virtual_media_grid::VirtualMediaGrid;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// Caps how many album rows one select-all may hold, matching PhotosPage.
const ALBUM_SELECT_ALL_LIMIT: u32 = 2_000;

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(resource = "/io/github/luyao_1024/photoviewer/ui/album-detail-page.ui")]
    pub struct AlbumDetailPage {
        pub media_list: RefCell<Option<gtk::gio::ListStore>>,
        pub master_media_list: RefCell<Option<gtk::gio::ListStore>>,
        pub pool: RefCell<Option<DbPool>>,
        pub db_actor: RefCell<Option<DbActorHandle>>,
        pub album: RefCell<Option<Album>>,
        pub nav_view: RefCell<Option<adw::NavigationView>>,
        pub loader: RefCell<Option<Arc<ThumbnailLoader>>>,
        pub grid: RefCell<Option<VirtualMediaGrid>>,
        /// Debounces media activation while NavigationView is pushing the
        /// viewer, matching PhotosPage's grid behavior.
        pub viewer_open_pending: Cell<bool>,
        /// Rejects stale background refreshes when several domain events land
        /// while the repository query is still running.
        pub refresh_generation: Cell<u64>,
        #[template_child]
        pub header_bar: TemplateChild<adw::HeaderBar>,
        #[template_child]
        pub search_btn: TemplateChild<gtk::Button>,
        #[template_child]
        pub select_all_revealer: TemplateChild<gtk::Revealer>,
        #[template_child]
        pub select_all_btn: TemplateChild<gtk::Button>,
        #[template_child]
        pub exit_multi_select_revealer: TemplateChild<gtk::Revealer>,
        #[template_child]
        pub exit_multi_select_btn: TemplateChild<gtk::Button>,
        #[template_child]
        pub add_to_album_revealer: TemplateChild<gtk::Revealer>,
        #[template_child]
        pub add_to_album_btn: TemplateChild<gtk::Button>,
        #[template_child]
        pub delete_to_trash_revealer: TemplateChild<gtk::Revealer>,
        #[template_child]
        pub delete_to_trash_btn: TemplateChild<gtk::Button>,
        #[template_child]
        pub selection_count_revealer: TemplateChild<gtk::Revealer>,
        #[template_child]
        pub selection_count_label: TemplateChild<gtk::Label>,
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
            crate::ensure_resources_registered();
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
    pub(crate) fn refresh_sync_badges(&self) {
        if let Some(grid) = self.imp().grid.borrow().as_ref() {
            grid.refresh_sync_badges();
        }
    }

    /// Build an `AlbumDetailPage` populated with a pre-filtered media list.
    /// The grid uses the same virtual Day grouping as `PhotosPage`.
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
                        this.open_viewer_for_media_id(media_id);
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
            let on_add_to_album: Rc<dyn Fn(Vec<MediaId>)> = {
                let weak = obj.downgrade();
                Rc::new(move |ids| {
                    if let Some(this) = weak.upgrade() {
                        this.open_album_picker_for_ids(ids);
                    }
                })
            };
            let grid = VirtualMediaGrid::new_for_query(
                media_list,
                media_query_for_album(
                    obj.imp()
                        .album
                        .borrow()
                        .as_ref()
                        .expect("album detail album initialized"),
                ),
                GroupBy::Day,
                loader,
                MediaGridCallbacks {
                    on_activate,
                    on_background_changed,
                    on_add_to_album,
                    on_move_to_trash,
                    on_set_favorite: Rc::new(|_, _| {}),
                    on_query_favorite_state: Rc::new(|_| FavoriteMenuState::default()),
                    on_set_album_cover: Some(on_set_album_cover),
                },
                true,
            );
            grid.set_context_menu_overlay(Some(&obj.imp().grid_overlay.get()));
            *obj.imp().grid.borrow_mut() = Some(grid.clone());
            obj.imp().content_box.get().append(&grid);
            let weak = obj.downgrade();
            grid.connect_selection_changed(move || {
                if let Some(this) = weak.upgrade() {
                    this.refresh_selection_ui();
                }
            });
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
        obj.wire_selection_chrome();
        obj
    }

    /// Labels and handlers for the header selection chrome. Copy comes from the
    /// same `photos.batch.*` keys the Photos header uses, so the two pages stay
    /// in step and no album-only wording drifts in.
    fn wire_selection_chrome(&self) {
        let tr = crate::core::i18n::tr;
        let imp = self.imp();
        imp.select_all_btn
            .get()
            .set_label(&tr("photos.batch.select_all"));
        imp.select_all_btn
            .get()
            .set_tooltip_text(Some(&tr("photos.batch.select_all")));
        imp.exit_multi_select_btn
            .get()
            .set_label(&tr("photos.batch.exit_multi_select"));
        imp.exit_multi_select_btn
            .get()
            .set_tooltip_text(Some(&tr("photos.batch.exit_multi_select")));
        imp.add_to_album_btn
            .get()
            .set_tooltip_text(Some(&tr("photos.add_to_album")));
        imp.delete_to_trash_btn
            .get()
            .set_tooltip_text(Some(&tr("viewer.tooltip.move_to_trash")));

        let weak = self.downgrade();
        imp.exit_multi_select_btn.get().connect_clicked(move |_| {
            if let Some(this) = weak.upgrade() {
                this.clear_selection();
            }
        });

        let weak = self.downgrade();
        imp.select_all_btn.get().connect_clicked(move |_| {
            let Some(this) = weak.upgrade() else {
                return;
            };
            this.toggle_album_select_all();
            this.refresh_selection_ui();
        });

        let weak = self.downgrade();
        imp.add_to_album_btn.get().connect_clicked(move |_| {
            let Some(this) = weak.upgrade() else {
                return;
            };
            let ids = this.selected_ids();
            if !ids.is_empty() {
                this.open_album_picker_for_ids(ids);
            }
        });

        let weak = self.downgrade();
        imp.delete_to_trash_btn.get().connect_clicked(move |_| {
            let Some(this) = weak.upgrade() else {
                return;
            };
            let ids = this.selected_ids();
            if !ids.is_empty() {
                this.delete_to_trash_for_ids(ids);
            }
        });
    }

    /// Every photo the album holds, bounded by the same cap PhotosPage uses.
    /// The grid's own `select_all()` only sees rows it has loaded — an album
    /// grid seeds one viewport and pages the rest by range — so answering
    /// select-all from it would quietly select a slice, or nothing at all when
    /// no row is resident yet. `None` means the album set is unknown (missing
    /// album/pool, a failed read, an empty album) and callers must not guess.
    fn album_select_all_ids(&self) -> Option<Vec<MediaId>> {
        let album = self.imp().album.borrow().as_ref().cloned()?;
        let pool = self.imp().pool.borrow().as_ref().cloned()?;
        let repo = MediaRepository::new(pool);
        let items = repo
            .items(media_query_for_album(&album), 0, ALBUM_SELECT_ALL_LIMIT)
            .ok()?;
        let ids = items
            .into_iter()
            .map(|item| MediaId::from(item.id))
            .collect::<Vec<_>>();
        (!ids.is_empty()).then_some(ids)
    }

    fn select_all_in_album(&self) {
        let Some(grid) = self.imp().grid.borrow().clone() else {
            return;
        };
        let has_context = self.imp().album.borrow().is_some() && self.imp().pool.borrow().is_some();
        match self.album_select_all_ids() {
            Some(ids) => grid.select_ids(&ids),
            // Without an album/pool context there is no album set to answer
            // from, and the grid's own window is the best effort left. A
            // failed read with context present stays a no-op so it cannot
            // clobber the selection with a slice.
            None if !has_context => grid.select_all(),
            None => {}
        }
    }

    /// The select-all button is a toggle, but its halves are not symmetric: a
    /// partial selection must be *completed* against the same capped album set
    /// `select_all_in_album` answers from, and only a selection that already
    /// covers that set clears. Judging by "is anything selected" made the
    /// button wipe a partially selected album instead of finishing it.
    fn toggle_album_select_all(&self) {
        let Some(grid) = self.imp().grid.borrow().clone() else {
            return;
        };
        let album = self.imp().album.borrow().as_ref().cloned();
        let pool = self.imp().pool.borrow().as_ref().cloned();
        let (Some(album), Some(pool)) = (album, pool) else {
            // No album context to measure a full set against; keep the
            // grid-local toggle the button used to answer with.
            if grid.selected_ids().is_empty() {
                grid.select_all();
            } else {
                grid.clear_selection();
            }
            return;
        };
        let repo = MediaRepository::new(pool);
        let Ok(items) = repo.items(media_query_for_album(&album), 0, ALBUM_SELECT_ALL_LIMIT) else {
            return; // a failed read must not clear what the user already selected
        };
        let ids = items
            .into_iter()
            .map(|item| MediaId::from(item.id))
            .collect::<Vec<_>>();
        if ids.is_empty() {
            return;
        }
        let selected_set: HashSet<MediaId> = grid.selected_ids().into_iter().collect();
        // Selection can only ever contain album rows, so "every capped id is
        // picked" means the user already holds everything select-all grants.
        if ids.iter().all(|id| selected_set.contains(id)) {
            grid.clear_selection();
        } else {
            grid.select_ids(&ids);
        }
    }

    fn selected_ids(&self) -> Vec<MediaId> {
        self.imp()
            .grid
            .borrow()
            .as_ref()
            .map(|grid| grid.selected_ids())
            .unwrap_or_default()
    }

    fn clear_selection(&self) {
        if let Some(grid) = self.imp().grid.borrow().as_ref() {
            grid.clear_selection();
        }
        self.refresh_selection_ui();
    }

    /// Reveal the batch actions only while something is selected, and swap the
    /// entry button for the exit button while multi-select is on. Mirrors
    /// `PhotosPage::refresh_selection_ui` for the controls this page has.
    fn refresh_selection_ui(&self) {
        let imp = self.imp();
        let binding = imp.grid.borrow();
        let Some(grid) = binding.as_ref() else {
            return;
        };
        let selected = grid.selected_ids();
        let has_any = !selected.is_empty();
        let multi = grid.is_multi_select_mode();
        imp.select_all_revealer.get().set_reveal_child(has_any);
        imp.add_to_album_revealer.get().set_reveal_child(has_any);
        imp.delete_to_trash_revealer.get().set_reveal_child(has_any);
        // Exit is bound to multi-select *mode*, not to having a selection, so
        // the user can always leave multi-select even after deselecting all.
        imp.exit_multi_select_revealer.get().set_reveal_child(multi);
        let label = if has_any {
            "photos.batch.unselect_all"
        } else {
            "photos.batch.select_all"
        };
        let text = crate::core::i18n::tr(label);
        imp.select_all_btn.get().set_label(&text);
        imp.select_all_btn.get().set_tooltip_text(Some(&text));
        // Same counter as PhotosPage's header: the batch buttons say what
        // happens, this says how many it happens to.
        let count = selected.len();
        let mut count_text =
            crate::core::i18n::trf("photos.selection.count", &[("n", &count.to_string())]);
        if count >= ALBUM_SELECT_ALL_LIMIT as usize {
            count_text.push_str(&crate::core::i18n::tr("photos.selection.limit"));
        }
        imp.selection_count_label.get().set_label(&count_text);
        imp.selection_count_revealer.get().set_reveal_child(has_any);
    }

    pub fn set_db_actor(&self, db_actor: DbActorHandle) {
        *self.imp().db_actor.borrow_mut() = Some(db_actor);
    }

    pub fn set_nav_target(&self, nav: &adw::NavigationView) {
        *self.imp().nav_view.borrow_mut() = Some(nav.clone());
    }

    fn open_album_picker_for_ids(&self, ids: Vec<MediaId>) {
        if ids.is_empty() {
            return;
        }
        let Some(nav) = self.imp().nav_view.borrow().as_ref().cloned() else {
            return;
        };
        let Some(pool) = self.imp().pool.borrow().as_ref().cloned() else {
            return;
        };
        let Some(db_actor) = self.imp().db_actor.borrow().as_ref().cloned() else {
            return;
        };
        let Some(loader) = self.imp().loader.borrow().as_ref().cloned() else {
            return;
        };
        album_picker::AlbumPickerDialog::present(
            &nav,
            pool,
            db_actor,
            loader,
            ids.into_iter().map(MediaId::get).collect(),
        );
    }

    pub(crate) fn handle_keyboard_action(&self, action: KeyboardAction) -> KeyboardResult {
        let grid = self.imp().grid.borrow().clone();
        if matches!(
            action,
            KeyboardAction::ActivateFocused | KeyboardAction::ToggleSelection
        ) {
            if let Some(grid) = grid.as_ref() {
                let result = grid.handle_keyboard_action(action);
                if result.is_handled() {
                    return result;
                }
            }
        }
        let Some(grid) = grid else {
            return KeyboardResult::Ignored;
        };
        // Same browsing-scope bindings PhotosPage answers to. The shortcuts
        // window advertises them for any photo list, so an album that silently
        // ignored Ctrl+A/Esc/Delete would be a lie once it has a selection.
        match action {
            KeyboardAction::SelectAll => {
                self.select_all_in_album();
                self.refresh_selection_ui();
                KeyboardResult::Handled
            }
            KeyboardAction::Delete => {
                let ids = self.selected_ids();
                if ids.is_empty() {
                    KeyboardResult::Ignored
                } else {
                    self.delete_to_trash_for_ids(ids);
                    KeyboardResult::Handled
                }
            }
            KeyboardAction::CancelOrClose => {
                if self.selected_ids().is_empty() && !grid.is_multi_select_mode() {
                    // Nothing to deselect: leave Escape to pop the navigation page.
                    KeyboardResult::Ignored
                } else {
                    self.clear_selection();
                    KeyboardResult::Handled
                }
            }
            _ => KeyboardResult::Ignored,
        }
    }

    fn delete_to_trash_for_ids(&self, ids: Vec<MediaId>) {
        if ids.is_empty() {
            return;
        }
        let trace = crate::core::telemetry::OperationTrace::start(
            crate::core::telemetry::TraceChain::Mutation,
            "move_to_trash",
        );
        let Some(db_actor) = self.imp().db_actor.borrow().as_ref().cloned() else {
            crate::core::telemetry::log_warning(
                &trace,
                "precondition",
                "DB actor is not initialized",
            );
            return;
        };
        let Some(pool) = self.imp().pool.borrow().as_ref().cloned() else {
            crate::core::telemetry::log_warning(
                &trace,
                "precondition",
                "database pool is not initialized",
            );
            return;
        };

        let weak = self.downgrade();
        let ids_for_worker = ids.clone();
        glib::spawn_future_local(async move {
            let prepared = db_actor
                .execute_in_trace(
                    trace.clone(),
                    DbCommand::MarkTrashed {
                        ids: ids_for_worker,
                    },
                )
                .await
                .inspect_err(|error| crate::core::telemetry::log_error(&trace, "db_mark", error))
                .ok();

            let Some(crate::core::DbCommandResult::MediaItems(items)) = prepared else {
                return;
            };
            if let Some(this) = weak.upgrade() {
                crate::ui::trash_fallback::move_marked_items_with_fallback(
                    &this,
                    pool,
                    db_actor,
                    items,
                    trace,
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
        let Some(db_actor) = self.imp().db_actor.borrow().as_ref().cloned() else {
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
                db_actor.execute_blocking(crate::core::db_actor::DbCommand::SetAlbumCover {
                    folder_path,
                    cover_uri,
                })
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

    fn open_viewer_for_media_id(&self, media_id: MediaId) {
        // Prefer the bounded shared window when it already contains the item so
        // the viewer opens with the full filmstrip populated (matching
        // PhotosPage::open_viewer). Only fall back to a single-item seed for a
        // virtual-range item outside that window; the Viewer then hydrates
        // neighbours through the album query.
        let media_list = self
            .imp()
            .media_list
            .borrow()
            .as_ref()
            .filter(|list| index_for_media_id(list, media_id).is_some())
            .cloned()
            .or_else(|| {
                self.imp()
                    .grid
                    .borrow()
                    .as_ref()
                    .and_then(|grid| grid.viewer_seed_for(media_id))
            });
        let Some(media_list) = media_list else {
            tracing::debug!(
                target: crate::core::log_targets::ALBUMS,
                media_id = media_id.get(),
                "AlbumDetailPage: ignoring viewer activation for an unready virtual slot"
            );
            return;
        };
        self.open_viewer(media_id, media_list);
    }

    fn open_viewer(&self, media_id: MediaId, media_list: gtk::gio::ListStore) {
        if self.imp().viewer_open_pending.get() {
            tracing::debug!(
                target: crate::core::log_targets::ALBUMS,
                "AlbumDetailPage: ignoring duplicate viewer activation while push is pending"
            );
            return;
        }

        let nav = match self.imp().nav_view.borrow().as_ref() {
            Some(n) => n.clone(),
            None => return,
        };
        if !self.is_visible() {
            tracing::debug!(
                target: crate::core::log_targets::ALBUMS,
                "AlbumDetailPage: ignoring viewer activation because AlbumDetailPage is not the visible browsing child"
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
        let initial_index = index_for_media_id(&media_list, media_id).unwrap_or(0);
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
        viewer.show_at(initial_index);

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

        let query = media_query_for_album(&album);
        let generation = self.imp().refresh_generation.get().saturating_add(1);
        self.imp().refresh_generation.set(generation);
        let weak = self.downgrade();

        // Repository count/page queries can block on SQLite or a busy disk.
        // Keep them off the GTK main thread and apply only the newest result.
        glib::spawn_future_local(async move {
            let fallback_total = album.photo_count;
            let result = gtk::gio::spawn_blocking(move || {
                let repo = MediaRepository::new(pool);
                let total = repo
                    .count(query.clone())
                    .map(i64::from)
                    .unwrap_or(fallback_total);
                let limit = album_refresh_load_limit(total);
                repo.items(query, 0, limit)
            })
            .await;
            let Some(this) = weak.upgrade() else {
                return;
            };
            if this.imp().refresh_generation.get() != generation {
                return;
            }
            let Ok(Ok(items)) = result else {
                tracing::warn!(
                    target: crate::core::log_targets::ALBUMS,
                    album_name = %album.display_name(),
                    album_path = %album.folder_path.display(),
                    "album_detail_page: repository refresh failed"
                );
                return;
            };
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
        });
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
        MediaQuery::MediaType(crate::core::media::LogicalMediaType::Animated)
    } else if album.is_hdr_album() {
        MediaQuery::MediaType(crate::core::media::LogicalMediaType::Hdr)
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
mod tests;
