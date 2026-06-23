//! PhotosPage: year/month/day view (shared MediaGrid, ModeSelector overlay).
//!
//! Hosts three MediaGrid instances. When the user clicks a tile, a `ViewerPage`
//! is pushed onto the host `AdwNavigationView` (injected via `set_nav_target`).
//! Shift/Ctrl-click on a tile multi-selects; the "Add to Album" toolbar button
//! appears whenever ≥1 tile is selected and opens the `AlbumPickerDialog`.
use std::cell::Ref;
use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::Arc;

use gtk4 as gtk;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::NavigationPageExt;

use crate::core::db::DbPool;
use crate::core::section_model::GroupBy;
use crate::core::thumbnails::ThumbnailLoader;
use crate::ui::album_picker;
use crate::ui::empty_states;
use crate::ui::media_grid::MediaGrid;
use crate::ui::mode_selector::ModeSelector;
use crate::ui::viewer_page::{NavDelta, ViewerPage, NAV_POP};

mod imp {
    use super::*;
    use adw::subclass::prelude::*;

    #[derive(gtk::CompositeTemplate)]
    #[template(file = "../../data/ui/photos-page.ui")]
    pub struct PhotosPage {
        pub media_list: RefCell<Option<gtk::gio::ListStore>>,
        pub loader: RefCell<Option<Arc<ThumbnailLoader>>>,
        pub nav_view: RefCell<Option<adw::NavigationView>>,
        pub pool: RefCell<Option<DbPool>>,
        /// Tracks the three MediaGrids so we can clear their selections and
        /// react to their `selection-changed` callbacks uniformly.
        pub grids: RefCell<Vec<MediaGrid>>,
        /// Global indices currently selected, in insertion order. Maintained
        /// by listening to each grid's `selection-changed` callback; not
        /// authoritative on its own — the per-grid `selected` set is.
        pub selected_indices: RefCell<HashSet<u32>>,
        #[template_child]
        pub header_bar: TemplateChild<adw::HeaderBar>,
        #[template_child]
        pub view_stack: TemplateChild<adw::ViewStack>,
        #[template_child]
        pub mode_selector: TemplateChild<ModeSelector>,
        #[template_child]
        pub add_to_album_btn: TemplateChild<gtk::Button>,
    }

    impl Default for PhotosPage {
        fn default() -> Self {
            Self {
                media_list: RefCell::new(None),
                loader: RefCell::new(None),
                nav_view: RefCell::new(None),
                pool: RefCell::new(None),
                grids: RefCell::new(Vec::new()),
                selected_indices: RefCell::new(HashSet::new()),
                header_bar: TemplateChild::default(),
                view_stack: TemplateChild::default(),
                mode_selector: TemplateChild::default(),
                add_to_album_btn: TemplateChild::default(),
            }
        }
    }

    #[gtk::glib::object_subclass]
    impl ObjectSubclass for PhotosPage {
        const NAME: &'static str = "PhotosPage";
        type Type = super::PhotosPage;
        type ParentType = adw::NavigationPage;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &gtk::glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for PhotosPage {}
    impl WidgetImpl for PhotosPage {}
    impl NavigationPageImpl for PhotosPage {}
}

gtk::glib::wrapper! {
    pub struct PhotosPage(ObjectSubclass<imp::PhotosPage>)
        @extends adw::NavigationPage, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable;
}

impl PhotosPage {
    /// Build a PhotosPage backed by `media_list`, sharing `loader` across the three
    /// mode-specific MediaGrids (Year/Month/Day).
    pub fn new(media_list: gtk::gio::ListStore, loader: Arc<ThumbnailLoader>) -> Self {
        let obj: Self = gtk::glib::Object::builder().build();
        *obj.imp().media_list.borrow_mut() = Some(media_list.clone());
        *obj.imp().loader.borrow_mut() = Some(loader.clone());

        // Snapshot the initial size before `media_list` is moved into MediaGrid.
        let is_empty = media_list.n_items() == 0;

        let on_activate: Rc<dyn Fn(u32)> = {
            let weak = obj.downgrade();
            Rc::new(move |global_index| {
                if let Some(this) = weak.upgrade() {
                    this.open_viewer(global_index);
                }
            })
        };
        let on_background_changed: Rc<dyn Fn()> = {
            let weak = obj.downgrade();
            Rc::new(move || {
                if let Some(this) = weak.upgrade() {
                    this.update_mode_selector_contrast();
                }
            })
        };

        // Three independent MediaGrid instances — one per grouping mode.
        let year_grid = MediaGrid::new(
            media_list.clone(),
            GroupBy::Year,
            loader.clone(),
            on_activate.clone(),
            on_background_changed.clone(),
        );
        let month_grid = MediaGrid::new(
            media_list.clone(),
            GroupBy::Month,
            loader.clone(),
            on_activate.clone(),
            on_background_changed.clone(),
        );
        let day_grid = MediaGrid::new(
            media_list,
            GroupBy::Day,
            loader,
            on_activate,
            on_background_changed,
        );

        // Wire selection-changed: each grid fires when its own `selected` set
        // changes. We collect the union into PhotosPage's `selected_indices`
        // and toggle the "Add to Album" button visibility. We use the union
        // (rather than per-grid bookkeeping) so the toolbar reflects the total
        // selected across year/month/day — important because only one mode
        // grid is visible at a time but the user may have multi-selected
        // before switching.
        {
            let weak = obj.downgrade();
            year_grid.connect_selection_changed(move || {
                if let Some(this) = weak.upgrade() {
                    this.refresh_selection_ui();
                }
            });
        }
        {
            let weak = obj.downgrade();
            month_grid.connect_selection_changed(move || {
                if let Some(this) = weak.upgrade() {
                    this.refresh_selection_ui();
                }
            });
        }
        {
            let weak = obj.downgrade();
            day_grid.connect_selection_changed(move || {
                if let Some(this) = weak.upgrade() {
                    this.refresh_selection_ui();
                }
            });
        }
        for grid in [&year_grid, &month_grid, &day_grid] {
            let weak = obj.downgrade();
            grid.connect_view_changed(move || {
                if let Some(this) = weak.upgrade() {
                    this.update_mode_selector_contrast();
                }
            });
        }
        *obj.imp().grids.borrow_mut() =
            vec![year_grid.clone(), month_grid.clone(), day_grid.clone()];

        let stack = obj.imp().view_stack.get();
        stack.add_titled(&year_grid, Some("year"), "年");
        stack.add_titled(&month_grid, Some("month"), "月");
        stack.add_titled(&day_grid, Some("day"), "日");

        // Empty-state placeholder: shown when the media list is empty.
        // Added as a hidden stack child so we can swap to it without rebuilding.
        let empty_page = empty_states::no_photos();
        empty_page.set_hexpand(true);
        empty_page.set_vexpand(true);
        stack.add(&empty_page); // untitled → won't appear in the switcher bar

        // Decide initial visible child based on data size.
        if is_empty {
            stack.set_visible_child(&empty_page);
        } else {
            stack.set_visible_child_name("day");
        }

        // Wire the ModeSelector to our view_stack (it drives the visible
        // child and reflects any external change back via notify).
        obj.imp().mode_selector.get().set_stack(&stack);
        {
            let weak = obj.downgrade();
            stack.connect_notify_local(Some("visible-child"), move |_, _| {
                if let Some(this) = weak.upgrade() {
                    this.schedule_mode_selector_contrast_update();
                }
            });
        }
        obj.schedule_mode_selector_contrast_update();

        // Wire the "Add to Album" toolbar button. Click → collect selected
        // ids → open AlbumPickerDialog. The dialog itself does the
        // copy/move + album refresh; we just need to provide the inputs.
        let weak = obj.downgrade();
        obj.imp().add_to_album_btn.get().connect_clicked(move |_| {
            let Some(this) = weak.upgrade() else {
                return;
            };
            let Some(nav) = this.imp().nav_view.borrow().clone() else {
                return;
            };
            let Some(pool) = this.imp().pool.borrow().clone() else {
                return;
            };
            let ids: Vec<i64> = this
                .imp()
                .selected_indices
                .borrow()
                .iter()
                .filter_map(|&gi| this.media_id_for_global_index(gi))
                .collect();
            if ids.is_empty() {
                return;
            }
            album_picker::AlbumPickerDialog::present(&nav, pool, ids);
        });

        obj
    }

    /// Inject the `AdwNavigationView` we live inside — needed to push/pop
    /// the viewer page. Called by the host (`app::build_app`) after pushing
    /// the PhotosPage.
    pub fn set_nav_target(&self, nav: &adw::NavigationView) {
        *self.imp().nav_view.borrow_mut() = Some(nav.clone());
    }

    /// Inject the `DbPool` so viewer pages can launch `EditorPage` with
    /// access to the database. Mirrors `set_nav_target`.
    pub fn set_db_pool(&self, pool: DbPool) {
        *self.imp().pool.borrow_mut() = Some(pool);
    }

    pub fn media_list(&self) -> Ref<'_, Option<gtk::gio::ListStore>> {
        self.imp().media_list.borrow()
    }

    /// Rebuild the union of selected indices from each visible grid, then
    /// show / hide the "Add to Album" button. Cheap (HashSet union) so it's
    /// fine to call on every `selection-changed` tick.
    fn refresh_selection_ui(&self) {
        let mut union: HashSet<u32> = HashSet::new();
        for grid in self.imp().grids.borrow().iter() {
            union.extend(grid.selected_indices());
        }
        let has_any = !union.is_empty();
        *self.imp().selected_indices.borrow_mut() = union;
        self.imp().add_to_album_btn.get().set_visible(has_any);
    }

    fn update_mode_selector_contrast(&self) {
        let selector = self.imp().mode_selector.get();
        let stack = self.imp().view_stack.get();
        let Some(visible) = stack.visible_child() else {
            selector.set_light_background(false);
            return;
        };
        let Some(grid) = visible.downcast_ref::<MediaGrid>() else {
            selector.set_light_background(false);
            return;
        };
        selector.set_light_background(grid.background_is_light_under(&selector).unwrap_or(false));
    }

    fn schedule_mode_selector_contrast_update(&self) {
        self.update_mode_selector_contrast();

        let weak = self.downgrade();
        glib::idle_add_local_once(move || {
            if let Some(this) = weak.upgrade() {
                this.update_mode_selector_contrast();
            }
        });

        let weak = self.downgrade();
        glib::timeout_add_local_once(std::time::Duration::from_millis(16), move || {
            if let Some(this) = weak.upgrade() {
                this.update_mode_selector_contrast();
            }
        });
    }

    /// Resolve a `MediaItem.id` for `global_index` by unwrapping the
    /// `BoxedAnyObject<MediaItem>` in the backing `ListStore`. Returns
    /// `None` if the store isn't set or the index is out of range.
    fn media_id_for_global_index(&self, gi: u32) -> Option<i64> {
        let list = self.imp().media_list.borrow();
        let list = list.as_ref()?;
        if gi >= list.n_items() {
            return None;
        }
        let obj = list.item(gi)?;
        // Borrow the inner MediaItem, copy out the id, then let the borrow
        // drop before the function returns (otherwise the temporary `Ref`
        // outlives the local `boxed` and the borrow checker complains).
        let boxed = obj.downcast::<glib::BoxedAnyObject>().ok()?;
        let id = boxed.borrow::<crate::core::media::MediaItem>().id;
        Some(id)
    }

    /// Clear selection across all three sub-grids and hide the toolbar.
    /// Called after a successful batch operation so the user can continue
    /// browsing without the previous selection leaking in.
    pub fn clear_selection(&self) {
        for grid in self.imp().grids.borrow().iter() {
            grid.clear_selection();
        }
        *self.imp().selected_indices.borrow_mut() = HashSet::new();
        self.imp().add_to_album_btn.get().set_visible(false);
    }

    fn open_viewer(&self, global_index: u32) {
        // Opening a viewer implicitly cancels any active multi-select — the
        // user is moving to single-photo mode. Otherwise stale selection
        // would persist after popping back, and a shift-click in the
        // viewer-mode area could re-add a stale index.
        self.clear_selection();

        let media_list = match self.imp().media_list.borrow().as_ref() {
            Some(l) => l.clone(),
            None => return,
        };
        let nav = match self.imp().nav_view.borrow().as_ref() {
            Some(n) => n.clone(),
            None => return,
        };
        let viewer_debug_label = format!("viewer-open-index-{global_index}");
        nav.connect_visible_page_notify({
            let label = viewer_debug_label.clone();
            move |nav| {
                tracing::info!(
                    "VIEWER_DEBUG nav visible_page_notify label={} visible={:?}",
                    label,
                    nav.visible_page().map(|page| page.title())
                );
            }
        });
        nav.connect_pushed({
            let label = viewer_debug_label.clone();
            move |nav| {
                tracing::info!(
                    "VIEWER_DEBUG nav pushed label={} visible={:?}",
                    label,
                    nav.visible_page().map(|page| page.title())
                );
            }
        });
        nav.connect_popped({
            let label = viewer_debug_label.clone();
            move |nav, page| {
                tracing::info!(
                    "VIEWER_DEBUG nav popped label={} popped_page={} visible_after={:?}",
                    label,
                    page.title(),
                    nav.visible_page().map(|page| page.title())
                );
            }
        });

        let viewer = ViewerPage::new(media_list, global_index);
        viewer.show_at(global_index);

        // Wire the viewer's Edit button: it pushes an EditorPage onto `nav`.
        if let Some(pool) = self.imp().pool.borrow().as_ref() {
            viewer.set_edit_target(&nav, pool.clone());
        }

        // Wire the viewer's "Add to Album" entry (single-photo).
        if let Some(pool) = self.imp().pool.borrow().as_ref() {
            viewer.set_album_target(&nav, pool.clone());
        }

        // Wire the viewer's keyboard callback: pops via the host NavigationView
        // for ESC, or advances/retreats the current index for ←/→.
        let viewer_weak = viewer.downgrade();
        let nav_weak = nav.downgrade();
        viewer.connect_navigation(move |delta: NavDelta| {
            tracing::info!(
                "VIEWER_DEBUG photos_page navigation_callback delta={}",
                delta
            );
            if delta == NAV_POP {
                if let Some(n) = nav_weak.upgrade() {
                    tracing::info!(
                        "VIEWER_DEBUG photos_page executing nav.pop visible_before={:?}",
                        n.visible_page().map(|page| page.title())
                    );
                    n.pop();
                    tracing::info!(
                        "VIEWER_DEBUG photos_page after nav.pop visible_after={:?}",
                        n.visible_page().map(|page| page.title())
                    );
                }
                return;
            }
            if let Some(v) = viewer_weak.upgrade() {
                let cur = v.current_index();
                let next = (cur as i32 + delta).max(0) as u32;
                tracing::info!(
                    "VIEWER_DEBUG photos_page arrow_nav cur={} delta={} next={}",
                    cur,
                    delta,
                    next
                );
                if let Some(list) = v.imp().media_list.borrow().as_ref() {
                    if next < list.n_items() {
                        v.show_at(next);
                    }
                }
            }
        });

        // Push the new viewer. Subsequent tile-clicks push a *new*
        // viewer; the previous one is reclaimed by the NavigationView
        // when the user pops back, so we don't need to track it here.
        nav.push(&viewer);
    }
}
