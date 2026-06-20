//! ViewerPage — fullscreen image viewer with preloading and gestures.
//!
//! `ViewerPage` is pushed onto the `AdwNavigationView` when the user clicks a
//! `PhotoTile`. It loads the `Large` thumbnail for the current item, plus
//! preloads the ±1 and ±2 neighbours so panning feels instant. It also wires
//! up a `GestureZoom` and a keyboard controller for basic interaction.
//!
//! Note: items in the `gio::ListStore` are `BoxedAnyObject<MediaItem>` (see
//! M1-T10 / `app::initialize`). We unwrap via `BoxedAnyObject::borrow` rather
//! than `downcast::<MediaItem>()`.
use crate::core::db::DbPool;
use crate::core::media::MediaItem;
use crate::core::thumbnails::{ThumbnailLoader, ThumbnailSize};
use crate::ui::editor_page::EditorPage;
use gtk4 as gtk;
use gtk4::gdk;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use libadwaita as adw;
use libadwaita::subclass::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

/// Direction hint the host receives from keyboard input. `i32::MIN` is the
/// "pop navigation" sentinel; other values are a delta on the current index.
pub type NavDelta = i32;
pub const NAV_POP: NavDelta = i32::MIN;

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(file = "../../data/ui/viewer-page.ui")]
    pub struct ViewerPage {
        pub media_list: RefCell<Option<gtk::gio::ListStore>>,
        pub current_index: Cell<u32>,
        pub loader: RefCell<Option<Arc<ThumbnailLoader>>>,
        /// Per-`show_at` token: any older response is dropped on arrival.
        pub current_token: Cell<u64>,
        /// Cumulative zoom scale (1.0 = identity). GestureZoom multiplies into it.
        pub zoom_scale: Cell<f64>,
        /// Callback registered by the host (PhotosPage) for keyboard navigation
        /// — needs the `loader` to fetch the next item.
        pub nav_cb: RefCell<Option<Rc<dyn Fn(NavDelta)>>>,
        /// Cached CssProvider reused across gesture ticks. Without this
        /// we would allocate a new provider on every pinch-tick and
        /// never release the previous one.
        pub zoom_provider: RefCell<Option<gtk::CssProvider>>,
        /// DB pool injected by host (needed to construct `EditorPage`).
        pub pool: RefCell<Option<DbPool>>,
        /// Navigation view used to push `EditorPage` when the user clicks Edit.
        pub nav_view: RefCell<Option<adw::NavigationView>>,
        #[template_child]
        pub header_bar: TemplateChild<adw::HeaderBar>,
        #[template_child]
        pub edit_btn: TemplateChild<gtk::Button>,
        #[template_child]
        pub picture: TemplateChild<gtk::Picture>,
        #[template_child]
        pub spinner: TemplateChild<gtk::Spinner>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ViewerPage {
        const NAME: &'static str = "ViewerPage";
        type Type = super::ViewerPage;
        type ParentType = adw::NavigationPage;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for ViewerPage {}
    impl WidgetImpl for ViewerPage {}
    impl NavigationPageImpl for ViewerPage {}
}

glib::wrapper! {
    pub struct ViewerPage(ObjectSubclass<imp::ViewerPage>)
        @extends adw::NavigationPage, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable;
}

impl ViewerPage {
    /// Build a new ViewerPage. Call `show_at(0, loader)` after construction
    /// to actually paint something.
    pub fn new(media_list: gtk::gio::ListStore, index: u32) -> Self {
        let obj: Self = glib::Object::builder().build();
        *obj.imp().media_list.borrow_mut() = Some(media_list);
        obj.imp().current_index.set(index);
        obj.setup_gesture();
        obj.setup_keyboard();
        obj.setup_edit_button();
        obj
    }

    /// Inject the `AdwNavigationView` and DB pool used to push an
    /// `EditorPage` when the Edit button is pressed. Call this after
    /// construction (mirrors `PhotosPage::set_nav_target`).
    pub fn set_edit_target(&self, nav: &adw::NavigationView, pool: DbPool) {
        *self.imp().nav_view.borrow_mut() = Some(nav.clone());
        *self.imp().pool.borrow_mut() = Some(pool);
    }

    /// Register a callback fired when the user presses ArrowLeft / ArrowRight /
    /// Escape. The callback receives the requested action: -1 / +1 / pop.
    pub fn connect_navigation<F: Fn(NavDelta) + 'static>(&self, f: F) {
        *self.imp().nav_cb.borrow_mut() = Some(Rc::new(f));
    }

    fn fire_nav(&self, delta: NavDelta) {
        let cb = self.imp().nav_cb.borrow().clone();
        if let Some(cb) = cb {
            cb(delta);
        }
    }

    /// Wire the Edit button: build an `EditorPage` for the currently
    /// displayed item, push it onto the host nav view, and wire its
    /// Cancel callback to pop the editor back to the viewer.
    fn setup_edit_button(&self) {
        let imp = self.imp();
        let weak = self.downgrade();
        imp.edit_btn.get().connect_clicked(move |_| {
            let Some(this) = weak.upgrade() else { return };
            let nav = match this.imp().nav_view.borrow().as_ref() {
                Some(n) => n.clone(),
                None => {
                    tracing::warn!("ViewerPage: Edit pressed but nav_view not set");
                    return;
                }
            };
            let pool = match this.imp().pool.borrow().as_ref() {
                Some(p) => p.clone(),
                None => {
                    tracing::warn!("ViewerPage: Edit pressed but pool not set");
                    return;
                }
            };
            let item = match this.current_media_item() {
                Some(i) => i,
                None => return,
            };
            let editor = EditorPage::new(item, pool);
            // When the user presses Cancel in the editor, pop it from the
            // nav view and return to the viewer.
            let nav_for_cancel = nav.downgrade();
            editor.connect_cancel(move || {
                if let Some(n) = nav_for_cancel.upgrade() {
                    n.pop();
                }
            });
            nav.push(&editor);
        });
    }

    /// Resolve the `MediaItem` at the current index out of the
    /// `BoxedAnyObject<MediaItem>` store. Returns `None` if the index is
    /// out of range or the item can't be downcast.
    fn current_media_item(&self) -> Option<MediaItem> {
        let list = self.imp().media_list.borrow();
        let list = list.as_ref()?;
        let idx = self.imp().current_index.get();
        if idx >= list.n_items() {
            return None;
        }
        let obj = list.item(idx)?;
        // Clone the `MediaItem` out of the `BoxedAnyObject` while both the
        // outer `Ref` (held by `list`) and `obj`/`boxed` are alive. Using
        // `match` (rather than `?`) makes the drop order explicit so the
        // borrow of `boxed` does not extend past its lifetime.
        let boxed = match obj.downcast::<glib::BoxedAnyObject>() {
            Ok(b) => b,
            Err(_) => return None,
        };
        let item = (*boxed.borrow::<MediaItem>()).clone();
        Some(item)
    }

    /// Display the item at `index`, load its `Large` thumbnail, and preload
    /// its immediate neighbours. Safe to call multiple times.
    pub fn show_at(&self, index: u32, loader: Arc<ThumbnailLoader>) {
        self.imp().current_index.set(index);
        *self.imp().loader.borrow_mut() = Some(loader.clone());
        self.imp().spinner.get().set_visible(true);

        // Bump token so a stale response from a previous show_at() doesn't
        // overwrite the current picture.
        let token = {
            let t = self.imp().current_token.get() + 1;
            self.imp().current_token.set(t);
            t
        };

        // Resolve the URI (BoxedAnyObject → MediaItem).
        let uri = {
            let list = self.imp().media_list.borrow();
            let list = match list.as_ref() {
                Some(l) => l,
                None => return,
            };
            if index >= list.n_items() {
                return;
            }
            match list.item(index).and_then(|o| o.downcast::<glib::BoxedAnyObject>().ok()) {
                Some(boxed) => boxed.borrow::<MediaItem>().uri.clone(),
                None => return,
            }
        };

        // Preload neighbours first (fire-and-forget — `request` is async, but
        // we just need the cache to be warm).
        self.preload_neighbor(-1, loader.clone());
        self.preload_neighbor(1, loader.clone());
        self.preload_neighbor(-2, loader.clone());
        self.preload_neighbor(2, loader.clone());

        // Load the current one and paint when it arrives.
        let (tx, rx) = tokio::sync::oneshot::channel();
        loader.request(uri, ThumbnailSize::Large, tx);

        let picture_weak = self.imp().picture.downgrade();
        let spinner_weak = self.imp().spinner.downgrade();
        let token_holder = self.imp().current_token.clone();
        glib::spawn_future_local(async move {
            let texture = match rx.await {
                Ok(t) => t,
                Err(_) => return, // sender dropped — cancelled
            };
            // Stale response: another show_at() ran in the meantime.
            if token_holder.get() != token {
                return;
            }
            if let (Some(picture), Some(spinner)) =
                (picture_weak.upgrade(), spinner_weak.upgrade())
            {
                picture.set_paintable(Some(&texture));
                spinner.set_visible(false);
            }
        });
    }

    /// Request a `Large` thumbnail for the neighbour at `current + offset`,
    /// if in range. The response is dropped (we only want the cache filled).
    pub fn preload_neighbor(&self, offset: i32, loader: Arc<ThumbnailLoader>) {
        let cur = self.imp().current_index.get() as i32;
        let target = cur + offset;
        let list = self.imp().media_list.borrow();
        let list = match list.as_ref() {
            Some(l) => l,
            None => return,
        };
        if target < 0 {
            return;
        }
        let target_u = target as u32;
        if target_u >= list.n_items() {
            return;
        }
        let Some(obj) = list.item(target_u) else {
            return;
        };
        let Ok(boxed) = obj.downcast::<glib::BoxedAnyObject>() else {
            return;
        };
        let uri = boxed.borrow::<MediaItem>().uri.clone();
        let (tx, rx) = tokio::sync::oneshot::channel();
        loader.request(uri, ThumbnailSize::Large, tx);
        // Keep the receiver alive until the worker delivers the texture.
        // Dropping `_rx` immediately would close the oneshot channel,
        // causing the worker to skip the cache fill (it sees the sender
        // closed) and we lose the preload benefit entirely.
        glib::spawn_future_local(async move {
            let _ = rx.await;
        });
    }

    fn setup_gesture(&self) {
        // Lazily allocate a single CssProvider and install it once on the
        // display. Subsequent gesture ticks only `load_from_data` to
        // update the transform, avoiding a fresh provider (and a leak)
        // on every pinch event.
        let provider = gtk::CssProvider::new();
        if let Some(display) = gtk::gdk::Display::default() {
            gtk::style_context_add_provider_for_display(
                &display,
                &provider,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }
        *self.imp().zoom_provider.borrow_mut() = Some(provider);

        let gesture = gtk::GestureZoom::new();
        let weak = self.downgrade();
        gesture.connect_scale_changed(move |_, scale| {
            if let Some(this) = weak.upgrade() {
                let prev = this.imp().zoom_scale.get();
                let next = (prev * scale).clamp(0.25, 8.0);
                this.imp().zoom_scale.set(next);

                // GtkPicture has no first-class `set_transform` API; we
                // express the accumulated scale via the shared stylesheet.
                // The picture re-paints on the next frame.
                let picture = this.imp().picture.get();
                if let Some(provider) = this.imp().zoom_provider.borrow().as_ref() {
                    let _ = provider.load_from_data(&format!(
                        "picture {{ transform: scale({}); }}",
                        next
                    ));
                }
                picture.queue_draw();
            }
        });
        self.imp().picture.get().add_controller(gesture);
    }

    fn setup_keyboard(&self) {
        let key_ctrl = gtk::EventControllerKey::new();
        let weak = self.downgrade();
        key_ctrl.connect_key_pressed(move |_, key, _, _| match key {
            gdk::Key::Right => {
                if let Some(this) = weak.upgrade() {
                    this.fire_nav(1);
                }
                glib::Propagation::Proceed
            }
            gdk::Key::Left => {
                if let Some(this) = weak.upgrade() {
                    this.fire_nav(-1);
                }
                glib::Propagation::Proceed
            }
            gdk::Key::Escape => {
                if let Some(this) = weak.upgrade() {
                    this.fire_nav(NAV_POP);
                }
                glib::Propagation::Proceed
            }
            _ => glib::Propagation::Proceed,
        });
        self.imp().picture.get().add_controller(key_ctrl);
    }

    /// Current item index in the backing `ListStore`.
    pub fn current_index(&self) -> u32 {
        self.imp().current_index.get()
    }
}
