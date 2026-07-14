//! Square thumbnail widget shared by media grids, albums, trash, and sidebar covers.

use gtk4 as gtk;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

mod imp {
    use super::*;

    pub struct SquareTile {
        /// A single manually-parented root lets a standard GtkOverlay own the
        /// picture and badge hierarchy while GridView recycles the tile.
        pub content: RefCell<Option<gtk::Overlay>>,
        pub picture: RefCell<Option<gtk::Picture>>,
        /// 半透明白色勾选标记，浮于缩略图右下角；始终 parented/allocated，
        /// 通过 CSS（flowboxchild:selected .thumb-checkmark）控制显隐，
        /// 仅在选中时可见。见 grid_css 的 .thumb-checkmark 规则。
        pub checkmark: RefCell<Option<gtk::Image>>,
        pub motion_badge: RefCell<Option<gtk::Image>>,
        pub duration_badge: RefCell<Option<gtk::Label>>,
        pub favorite_badge: RefCell<Option<gtk::Image>>,
        pub target: Cell<i32>,
        /// Virtual GridView cells may lose a few pixels to the scroller or
        /// CSS box model after their column count has been chosen. Keep the
        /// target as the natural size while allowing that final allocation to
        /// shrink instead of rejecting GTK's measure request.
        pub allow_width_shrink: Cell<bool>,
        pub background_is_light: Cell<Option<bool>>,
        /// 该 tile 的缩略图缓存键（建 tile 时预算，用于可见区提权匹配队列项）。
        pub cache_key: RefCell<Option<String>>,
        pub thumbnail_request: RefCell<Option<Rc<dyn Fn()>>>,
    }

    impl Default for SquareTile {
        fn default() -> Self {
            Self {
                content: RefCell::new(None),
                picture: RefCell::new(None),
                checkmark: RefCell::new(None),
                motion_badge: RefCell::new(None),
                duration_badge: RefCell::new(None),
                favorite_badge: RefCell::new(None),
                target: Cell::new(90),
                allow_width_shrink: Cell::new(false),
                background_is_light: Cell::new(None),
                cache_key: RefCell::new(None),
                thumbnail_request: RefCell::new(None),
            }
        }
    }

    #[gtk::glib::object_subclass]
    impl ObjectSubclass for SquareTile {
        const NAME: &'static str = "PvSquareTile";
        type Type = super::SquareTile;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for SquareTile {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            obj.add_css_class("thumb-tile");
            obj.add_css_class("glass-thumb-card");
            obj.set_overflow(gtk::Overflow::Hidden);

            let content = gtk::Overlay::new();
            content.set_hexpand(true);
            content.set_vexpand(true);
            content.set_parent(&*obj);
            *self.content.borrow_mut() = Some(content.clone());

            let picture = gtk::Picture::builder()
                .content_fit(gtk::ContentFit::Cover)
                .can_shrink(true)
                .build();
            picture.add_css_class("thumb-image");
            content.set_child(Some(&picture));
            *self.picture.borrow_mut() = Some(picture);

            // Selection checkmark: a translucent-white tick pinned to the
            // bottom-right, drawn above the picture. It is always
            // parented/allocated but invisible (opacity 0) until the
            // wrapping FlowBoxChild becomes :selected, when CSS reveals it.
            let checkmark = gtk::Image::builder()
                .icon_name("object-select-symbolic")
                .pixel_size(22)
                .build();
            checkmark.add_css_class("thumb-checkmark");
            checkmark.set_halign(gtk::Align::End);
            checkmark.set_valign(gtk::Align::End);
            checkmark.set_margin_end(6);
            checkmark.set_margin_bottom(6);
            content.add_overlay(&checkmark);
            *self.checkmark.borrow_mut() = Some(checkmark);

            let motion_badge = gtk::Image::builder()
                .icon_name("media-playback-start-symbolic")
                .pixel_size(18)
                .visible(false)
                .build();
            motion_badge.add_css_class("thumb-motion-badge");
            motion_badge.set_halign(gtk::Align::Start);
            motion_badge.set_valign(gtk::Align::End);
            motion_badge.set_margin_start(7);
            motion_badge.set_margin_bottom(7);
            content.add_overlay(&motion_badge);
            *self.motion_badge.borrow_mut() = Some(motion_badge);

            let duration_badge = gtk::Label::builder()
                .visible(false)
                .halign(gtk::Align::Start)
                .valign(gtk::Align::End)
                .build();
            duration_badge.add_css_class("thumb-video-duration");
            duration_badge.set_margin_start(7);
            duration_badge.set_margin_bottom(7);
            content.add_overlay(&duration_badge);
            *self.duration_badge.borrow_mut() = Some(duration_badge);

            let favorite_badge = gtk::Image::builder()
                .icon_name("emblem-favorite-symbolic")
                .pixel_size(20)
                .visible(false)
                .build();
            favorite_badge.add_css_class("thumb-favorite-badge");
            favorite_badge.set_halign(gtk::Align::End);
            favorite_badge.set_valign(gtk::Align::Start);
            favorite_badge.set_margin_end(7);
            favorite_badge.set_margin_top(7);
            content.add_overlay(&favorite_badge);
            *self.favorite_badge.borrow_mut() = Some(favorite_badge);
        }

        fn dispose(&self) {
            if let Some(content) = self.content.borrow_mut().take() {
                content.unparent();
            }
            self.picture.borrow_mut().take();
            self.checkmark.borrow_mut().take();
            self.motion_badge.borrow_mut().take();
            self.duration_badge.borrow_mut().take();
            self.favorite_badge.borrow_mut().take();
        }
    }

    impl WidgetImpl for SquareTile {
        fn request_mode(&self) -> gtk::SizeRequestMode {
            // GridView asks every realized child for its row height at the
            // allocated column width. Without this declaration GTK treats the
            // tile as constant-size, passes `-1` here, and leaves a resized
            // column paired with the old fixed-height tile.
            gtk::SizeRequestMode::HeightForWidth
        }

        // `target` is the preferred square size in the unconstrained
        // direction. With a real column width, HeightForWidth returns that
        // width so the tile stays square as GridView reflows. Virtual grid
        // cells retain this as their natural width but may advertise a
        // smaller minimum: their final allocated column can be a couple of
        // pixels narrower than the viewport-derived layout target.
        // NB: do NOT set a layout manager here — GTK4 would then measure via
        // the layout manager and bypass this override.
        fn measure(&self, orientation: gtk::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            let target = self.target.get().max(1);
            if orientation == gtk::Orientation::Horizontal {
                let minimum = if self.allow_width_shrink.get() {
                    1
                } else {
                    target
                };
                return (minimum, target, -1, -1);
            }

            let size = if for_size > 0 { for_size } else { target };
            (size, size, -1, -1)
        }

        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            // A GtkGridView can briefly deallocate a recycled child while it
            // recalculates columns. GTK permits a zero allocation at that
            // point, whereas `i32::clamp(1, 0)` panics. Keep every child
            // allocation non-negative until the next normal layout pass.
            let width = width.max(0);
            let height = height.max(0);
            if let Some(content) = self.content.borrow().as_ref() {
                content.size_allocate(&gtk::Allocation::new(0, 0, width, height), baseline);
            }
        }
    }
}

gtk::glib::wrapper! {
    pub struct SquareTile(ObjectSubclass<imp::SquareTile>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Default for SquareTile {
    fn default() -> Self {
        Self::new()
    }
}

impl SquareTile {
    pub fn new() -> Self {
        gtk::glib::Object::builder().build()
    }

    pub fn set_target(&self, target: i32) {
        self.imp().target.set(target);
        self.queue_resize();
    }

    pub fn target(&self) -> i32 {
        self.imp().target.get()
    }

    /// Let a virtual `GtkGridView` allocate this square at a width slightly
    /// below its preferred target. Other grids retain the default fixed
    /// minimum width so their existing layout contracts are unchanged.
    pub fn set_allow_width_shrink(&self, allow: bool) {
        if self.imp().allow_width_shrink.replace(allow) != allow {
            self.queue_resize();
        }
    }

    pub fn allows_width_shrink(&self) -> bool {
        self.imp().allow_width_shrink.get()
    }

    pub fn set_paintable<P: IsA<gtk::gdk::Paintable>>(&self, paintable: Option<&P>) {
        if let Some(p) = self.imp().picture.borrow().as_ref() {
            p.set_paintable(paintable);
        }
        // 设入任意 paintable（真实 texture 或失败灰底）即停止骨架 shimmer。
        // 透明度由 CSS 驱动（普通 .thumb-loading 隐藏，.thumb-placeholder
        // 保持可见；.glass-thumb-card 的 opacity transition 负责淡入）。
        // 不要在此处 widget.set_opacity，否则会绕过 CSS transition 直接跳变。
        self.remove_css_class("thumb-loading");
        self.remove_css_class("thumb-placeholder");
        // Legacy FlowBox grids fade their wrapper itself. Restore only that
        // known wrapper once a texture arrives. A GtkGridView uses a private
        // list-item wrapper here; changing its opacity during factory bind is
        // re-entrant with GTK's accessibility bookkeeping.
        if let Some(flow_child) = self.parent().and_downcast::<gtk::FlowBoxChild>() {
            flow_child.set_opacity(1.0);
        }
    }

    /// Reset every piece of media-specific state before a `GtkListItem` reuses
    /// this tile for another virtual-grid slot.  This deliberately does not
    /// perform cache I/O: the next bind decides whether the tile is a filler,
    /// placeholder, or a ready media item and may then start an async request.
    pub fn clear_for_rebind(&self) {
        if let Some(picture) = self.imp().picture.borrow().as_ref() {
            picture.set_paintable(None::<&gtk::gdk::Paintable>);
        }
        self.imp().background_is_light.set(None);
        *self.imp().cache_key.borrow_mut() = None;
        *self.imp().thumbnail_request.borrow_mut() = None;
        self.set_motion_badge_visible(false);
        self.set_video_duration(None);
        self.set_favorite_badge_visible(false);
        self.remove_css_class("thumb-loading");
        self.remove_css_class("thumb-placeholder");
        self.remove_css_class("media-selected");
        self.set_opacity(1.0);
        self.set_visible(true);
        self.set_can_target(true);
    }

    /// Put a freshly rebound tile into the stable loading state.  The virtual
    /// GridView intentionally keeps this placeholder visible while its worker
    /// checks disk cache / decodes; callers must use `set_paintable` only once
    /// they have a concrete texture or unavailable fallback.
    pub fn show_loading_placeholder(&self) {
        self.add_css_class("thumb-loading");
        self.add_css_class("thumb-placeholder");
    }

    pub fn set_background_is_light(&self, is_light: bool) {
        self.imp().background_is_light.set(Some(is_light));
    }

    pub fn background_is_light(&self) -> Option<bool> {
        self.imp().background_is_light.get()
    }

    /// 缩略图缓存键（建 tile 时预算；可见区提权时用它匹配队列项）。
    pub fn set_cache_key(&self, key: Option<String>) {
        *self.imp().cache_key.borrow_mut() = key;
    }

    pub fn cache_key(&self) -> Option<String> {
        self.imp().cache_key.borrow().clone()
    }

    pub fn set_thumbnail_request(&self, request: Rc<dyn Fn()>) {
        *self.imp().thumbnail_request.borrow_mut() = Some(request);
    }

    pub fn request_thumbnail(&self) {
        if let Some(request) = self.imp().thumbnail_request.borrow().as_ref() {
            request();
        }
    }

    pub fn set_motion_badge_visible(&self, visible: bool) {
        if let Some(badge) = self.imp().motion_badge.borrow().as_ref() {
            badge.set_visible(visible);
        }
    }

    pub fn set_video_duration(&self, label: Option<&str>) {
        if let Some(badge) = self.imp().duration_badge.borrow().as_ref() {
            badge.set_label(label.unwrap_or(""));
            badge.set_visible(label.is_some());
        }
    }

    pub fn set_favorite_badge_visible(&self, visible: bool) {
        if let Some(badge) = self.imp().favorite_badge.borrow().as_ref() {
            badge.set_visible(visible);
        }
    }
}

#[cfg(test)]
mod tests;
