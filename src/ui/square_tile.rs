//! Square thumbnail widget shared by media grids, albums, trash, and sidebar covers.

use gtk4 as gtk;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

mod imp {
    use super::*;

    pub struct SquareTile {
        pub picture: RefCell<Option<gtk::Picture>>,
        /// 半透明白色勾选标记，浮于缩略图右下角；始终 parented/allocated，
        /// 通过 CSS（flowboxchild:selected .thumb-checkmark）控制显隐，
        /// 仅在选中时可见。见 grid_css 的 .thumb-checkmark 规则。
        pub checkmark: RefCell<Option<gtk::Image>>,
        pub motion_badge: RefCell<Option<gtk::Image>>,
        pub duration_badge: RefCell<Option<gtk::Label>>,
        pub favorite_badge: RefCell<Option<gtk::Image>>,
        pub target: Cell<i32>,
        pub background_is_light: Cell<Option<bool>>,
        /// 该 tile 的缩略图缓存键（建 tile 时预算，用于可见区提权匹配队列项）。
        pub cache_key: RefCell<Option<String>>,
        pub thumbnail_request: RefCell<Option<Rc<dyn Fn()>>>,
    }

    impl Default for SquareTile {
        fn default() -> Self {
            Self {
                picture: RefCell::new(None),
                checkmark: RefCell::new(None),
                motion_badge: RefCell::new(None),
                duration_badge: RefCell::new(None),
                favorite_badge: RefCell::new(None),
                target: Cell::new(90),
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
            let picture = gtk::Picture::builder()
                .content_fit(gtk::ContentFit::Cover)
                .can_shrink(true)
                .build();
            obj.add_css_class("thumb-tile");
            obj.add_css_class("glass-thumb-card");
            obj.set_overflow(gtk::Overflow::Hidden);
            picture.add_css_class("thumb-image");
            picture.set_parent(&*obj);
            *self.picture.borrow_mut() = Some(picture);

            // Selection checkmark: a translucent-white tick pinned to the
            // bottom-right, drawn above the picture. It is always
            // parented/allocated but invisible (opacity 0) until the
            // wrapping FlowBoxChild becomes :selected, when CSS reveals it.
            // Parented after the picture so GTK draws it on top.
            let checkmark = gtk::Image::builder()
                .icon_name("object-select-symbolic")
                .pixel_size(22)
                .build();
            checkmark.add_css_class("thumb-checkmark");
            checkmark.set_parent(&*obj);
            *self.checkmark.borrow_mut() = Some(checkmark);

            let motion_badge = gtk::Image::builder()
                .icon_name("media-playback-start-symbolic")
                .pixel_size(18)
                .visible(false)
                .build();
            motion_badge.add_css_class("thumb-motion-badge");
            motion_badge.set_parent(&*obj);
            *self.motion_badge.borrow_mut() = Some(motion_badge);

            let duration_badge = gtk::Label::builder()
                .visible(false)
                .halign(gtk::Align::Start)
                .valign(gtk::Align::End)
                .build();
            duration_badge.add_css_class("thumb-video-duration");
            duration_badge.set_parent(&*obj);
            *self.duration_badge.borrow_mut() = Some(duration_badge);

            let favorite_badge = gtk::Image::builder()
                .icon_name("emblem-favorite-symbolic")
                .pixel_size(20)
                .visible(false)
                .build();
            favorite_badge.add_css_class("thumb-favorite-badge");
            favorite_badge.set_parent(&*obj);
            *self.favorite_badge.borrow_mut() = Some(favorite_badge);
        }

        fn dispose(&self) {
            if let Some(p) = self.picture.borrow_mut().take() {
                p.unparent();
            }
            if let Some(c) = self.checkmark.borrow_mut().take() {
                c.unparent();
            }
            if let Some(b) = self.motion_badge.borrow_mut().take() {
                b.unparent();
            }
            if let Some(d) = self.duration_badge.borrow_mut().take() {
                d.unparent();
            }
            if let Some(f) = self.favorite_badge.borrow_mut().take() {
                f.unparent();
            }
        }
    }

    impl WidgetImpl for SquareTile {
        // Fixed square size: `target` in both orientations (height-for-width
        // returns the given width, so it stays square at any column size).
        // NB: do NOT set a layout manager here — GTK4 would then measure via
        // the layout manager and bypass this override.
        fn measure(&self, orientation: gtk::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            let target = self.target.get().max(1);
            let size = if orientation == gtk::Orientation::Vertical && for_size > 0 {
                for_size
            } else {
                target
            };
            (size, size, -1, -1)
        }

        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            if let Some(p) = self.picture.borrow().as_ref() {
                p.size_allocate(&gtk::Allocation::new(0, 0, width, height), baseline);
            }
            // Pin the checkmark to the bottom-right corner with a small
            // margin. Use its natural (pixel-size) extent, clamped to the
            // tile so it never overflows the clipped card.
            if let Some(c) = self.checkmark.borrow().as_ref() {
                let (_, cw, _, _) = c.measure(gtk::Orientation::Horizontal, -1);
                let (_, ch, _, _) = c.measure(gtk::Orientation::Vertical, -1);
                let cw = cw.clamp(1, width);
                let ch = ch.clamp(1, height);
                let margin = 6;
                let x = (width - cw - margin).max(0);
                let y = (height - ch - margin).max(0);
                c.size_allocate(&gtk::Allocation::new(x, y, cw, ch), -1);
            }
            if let Some(b) = self.motion_badge.borrow().as_ref() {
                let (_, bw, _, _) = b.measure(gtk::Orientation::Horizontal, -1);
                let (_, bh, _, _) = b.measure(gtk::Orientation::Vertical, -1);
                let bw = bw.clamp(1, width);
                let bh = bh.clamp(1, height);
                let margin = 7;
                let y = (height - bh - margin).max(0);
                b.size_allocate(&gtk::Allocation::new(margin, y, bw, bh), -1);
            }
            if let Some(d) = self.duration_badge.borrow().as_ref() {
                let (_, dw, _, _) = d.measure(gtk::Orientation::Horizontal, -1);
                let (_, dh, _, _) = d.measure(gtk::Orientation::Vertical, -1);
                let dw = dw.clamp(1, width);
                let dh = dh.clamp(1, height);
                let margin = 7;
                let y = (height - dh - margin).max(0);
                d.size_allocate(&gtk::Allocation::new(margin, y, dw, dh), -1);
            }
            if let Some(f) = self.favorite_badge.borrow().as_ref() {
                let (_, fw, _, _) = f.measure(gtk::Orientation::Horizontal, -1);
                let (_, fh, _, _) = f.measure(gtk::Orientation::Vertical, -1);
                let fw = fw.clamp(1, width);
                let fh = fh.clamp(1, height);
                let margin = 7;
                let x = (width - fw - margin).max(0);
                f.size_allocate(&gtk::Allocation::new(x, margin, fw, fh), -1);
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
        // 父 FlowBoxChild 在 sync_flow_child_visibility_for_tile 里随
        // loading 状态同步到父 FlowBoxChild；纹理到位时立刻把它恢复可见，
        // 好让上面的 tile 淡入能被看到。
        if let Some(parent) = self.parent() {
            parent.set_opacity(1.0);
        }
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
