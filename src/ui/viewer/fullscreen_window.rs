use super::fullscreen::viewer_overlay_button;
use super::transform::step_zoom;
use super::ViewerPage;
use crate::core::i18n::tr;
use gtk4 as gtk;
use gtk4::gdk;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::subclass::prelude::ObjectSubclassIsExt;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

impl ViewerPage {
    pub(super) fn setup_fullscreen_button(&self) {
        self.update_fullscreen_button();
        let weak = self.downgrade();
        self.imp().fullscreen_btn.get().connect_clicked(move |_| {
            let Some(this) = weak.upgrade() else {
                return;
            };
            this.open_fullscreen_preview_window();
        });
    }

    pub(super) fn open_fullscreen_preview_window(&self) {
        if let Some(window) = self.imp().fullscreen_preview_window.borrow().as_ref() {
            window.present();
            return;
        }

        let Some(paintable) = self.imp().picture.get().paintable() else {
            return;
        };

        let title = self
            .current_media_item()
            .map(|item| item.display_name().to_string())
            .unwrap_or_else(|| tr("page.viewer.title"));
        let parent_window = self
            .root()
            .and_then(|root| root.downcast::<gtk::Window>().ok());
        let (default_width, default_height) = parent_window
            .as_ref()
            .and_then(|parent| {
                let surface = parent.surface()?;
                let display = gtk::prelude::WidgetExt::display(parent);
                let monitor = display.monitor_at_surface(&surface)?;
                let geometry = monitor.geometry();
                Some((geometry.width(), geometry.height()))
            })
            .unwrap_or((1024, 768));
        let window = gtk::Window::builder()
            .title(title.as_str())
            .default_width(default_width)
            .default_height(default_height)
            .decorated(false)
            .fullscreened(true)
            .build();
        if let Some(application) = parent_window
            .as_ref()
            .and_then(|parent| parent.application())
        {
            window.set_application(Some(&application));
        }

        let overlay = gtk::Overlay::new();
        overlay.add_css_class("viewer-stage");

        let picture = gtk::Picture::builder()
            .paintable(&paintable)
            .content_fit(gtk::ContentFit::Contain)
            .can_shrink(true)
            .hexpand(true)
            .vexpand(true)
            .build();
        picture.add_css_class("viewer-media-surface");
        picture.add_css_class("viewer-fullscreen-preview-picture");
        overlay.set_child(Some(&picture));

        let pic_for_fade = picture.downgrade();

        let preview_provider = Rc::new(gtk::CssProvider::new());
        gtk::style_context_add_provider_for_display(
            &gtk::prelude::WidgetExt::display(&picture),
            preview_provider.as_ref(),
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
        let preview_scale = Rc::new(Cell::new(super::MIN_VIEWER_ZOOM));
        let preview_rotation = Rc::new(Cell::new(0_i32));

        let nav_controls = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(4)
            .halign(gtk::Align::End)
            .valign(gtk::Align::End)
            .margin_bottom(34)
            .margin_end(10)
            .build();
        nav_controls.add_css_class("viewer-overlay-nav");
        let preview_prev_btn =
            viewer_overlay_button("go-previous-symbolic", &tr("viewer.tooltip.previous"));
        let preview_next_btn =
            viewer_overlay_button("go-next-symbolic", &tr("viewer.tooltip.next"));
        nav_controls.append(&preview_prev_btn);
        nav_controls.append(&preview_next_btn);
        overlay.add_overlay(&nav_controls);

        let zoom_controls = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(4)
            .halign(gtk::Align::End)
            .valign(gtk::Align::Start)
            .margin_top(10)
            .margin_end(10)
            .build();
        zoom_controls.add_css_class("viewer-zoom-controls");
        let preview_zoom_reset_btn =
            viewer_overlay_button("zoom-fit-best-symbolic", &tr("viewer.tooltip.zoom_reset"));
        let preview_zoom_out_btn =
            viewer_overlay_button("zoom-out-symbolic", &tr("viewer.tooltip.zoom_out"));
        let preview_rotate_left_btn = viewer_overlay_button(
            "object-rotate-left-symbolic",
            &tr("viewer.tooltip.rotate_left"),
        );
        let preview_rotate_right_btn = viewer_overlay_button(
            "object-rotate-right-symbolic",
            &tr("viewer.tooltip.rotate_right"),
        );
        let preview_restore_btn = viewer_overlay_button(
            "view-restore-symbolic",
            &tr("viewer.tooltip.exit_fullscreen"),
        );
        let preview_zoom_in_btn =
            viewer_overlay_button("zoom-in-symbolic", &tr("viewer.tooltip.zoom_in"));
        zoom_controls.append(&preview_zoom_reset_btn);
        zoom_controls.append(&preview_zoom_out_btn);
        zoom_controls.append(&preview_rotate_left_btn);
        zoom_controls.append(&preview_rotate_right_btn);
        zoom_controls.append(&preview_restore_btn);
        zoom_controls.append(&preview_zoom_in_btn);
        overlay.add_overlay(&zoom_controls);
        window.set_child(Some(&overlay));

        let update_preview_controls: Rc<dyn Fn()> = Rc::new({
            let picture = picture.clone();
            let provider = preview_provider.clone();
            let scale = preview_scale.clone();
            let rotation = preview_rotation.clone();
            let zoom_reset_btn = preview_zoom_reset_btn.clone();
            let zoom_out_btn = preview_zoom_out_btn.clone();
            let rotate_left_btn = preview_rotate_left_btn.clone();
            let rotate_right_btn = preview_rotate_right_btn.clone();
            let zoom_in_btn = preview_zoom_in_btn.clone();
            move || {
                let current_scale = scale.get();
                let current_rotation = rotation.get();
                provider.load_from_data(&format!(
                    "picture.viewer-fullscreen-preview-picture {{ transform: rotate({current_rotation}deg) scale({current_scale}); }}"
                ));
                let zoomed = current_scale > super::MIN_VIEWER_ZOOM;
                zoom_in_btn.set_visible(true);
                zoom_out_btn.set_visible(zoomed);
                zoom_reset_btn.set_visible(zoomed);
                rotate_left_btn.set_visible(!zoomed);
                rotate_right_btn.set_visible(!zoomed);
                picture.queue_draw();
            }
        });
        update_preview_controls();

        let paintable_handler_id: Rc<RefCell<Option<glib::SignalHandlerId>>> =
            Rc::new(RefCell::new(None));
        let handler_id = self.imp().picture.get().connect_paintable_notify({
            let preview_picture = picture.downgrade();
            let preview_scale = preview_scale.clone();
            let preview_rotation = preview_rotation.clone();
            let update_preview_controls = update_preview_controls.clone();
            move |main_picture| {
                let Some(preview_picture) = preview_picture.upgrade() else {
                    return;
                };
                if let Some(paintable) = main_picture.paintable() {
                    preview_picture.set_paintable(Some(&paintable));
                    preview_scale.set(super::MIN_VIEWER_ZOOM);
                    preview_rotation.set(0);
                    update_preview_controls();
                }
            }
        });
        *paintable_handler_id.borrow_mut() = Some(handler_id);

        let weak = self.downgrade();
        let preview_scale_for_prev = preview_scale.clone();
        let preview_rotation_for_prev = preview_rotation.clone();
        let update_for_prev = update_preview_controls.clone();
        preview_prev_btn.connect_clicked(move |_| {
            if let Some(this) = weak.upgrade() {
                preview_scale_for_prev.set(super::MIN_VIEWER_ZOOM);
                preview_rotation_for_prev.set(0);
                update_for_prev();
                this.navigate_by_delta(-1);
            }
        });
        let weak = self.downgrade();
        let preview_scale_for_next = preview_scale.clone();
        let preview_rotation_for_next = preview_rotation.clone();
        let update_for_next = update_preview_controls.clone();
        preview_next_btn.connect_clicked(move |_| {
            if let Some(this) = weak.upgrade() {
                preview_scale_for_next.set(super::MIN_VIEWER_ZOOM);
                preview_rotation_for_next.set(0);
                update_for_next();
                this.navigate_by_delta(1);
            }
        });

        let scale_for_zoom_in = preview_scale.clone();
        let update_for_zoom_in = update_preview_controls.clone();
        preview_zoom_in_btn.connect_clicked(move |_| {
            scale_for_zoom_in.set(step_zoom(scale_for_zoom_in.get(), 1));
            update_for_zoom_in();
        });
        let scale_for_zoom_out = preview_scale.clone();
        let update_for_zoom_out = update_preview_controls.clone();
        preview_zoom_out_btn.connect_clicked(move |_| {
            scale_for_zoom_out.set(step_zoom(scale_for_zoom_out.get(), -1));
            update_for_zoom_out();
        });
        let scale_for_reset = preview_scale.clone();
        let rotation_for_reset = preview_rotation.clone();
        let update_for_reset = update_preview_controls.clone();
        preview_zoom_reset_btn.connect_clicked(move |_| {
            scale_for_reset.set(super::MIN_VIEWER_ZOOM);
            rotation_for_reset.set(0);
            update_for_reset();
        });
        let rotation_for_left = preview_rotation.clone();
        let update_for_left = update_preview_controls.clone();
        preview_rotate_left_btn.connect_clicked(move |_| {
            rotation_for_left.set((rotation_for_left.get() - 90).rem_euclid(360));
            update_for_left();
        });
        let rotation_for_right = preview_rotation.clone();
        let update_for_right = update_preview_controls.clone();
        preview_rotate_right_btn.connect_clicked(move |_| {
            rotation_for_right.set((rotation_for_right.get() + 90).rem_euclid(360));
            update_for_right();
        });
        let window_weak = window.downgrade();
        preview_restore_btn.connect_clicked(move |_| {
            if let Some(window) = window_weak.upgrade() {
                window.close();
            }
        });

        let key = gtk::EventControllerKey::new();
        let window_weak = window.downgrade();
        key.connect_key_pressed(move |_, key, _, _| {
            if key == gdk::Key::Escape {
                if let Some(window) = window_weak.upgrade() {
                    window.close();
                }
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        window.add_controller(key);

        let weak = self.downgrade();
        let handler_id_for_close = paintable_handler_id.clone();
        window.connect_close_request(move |_| {
            if let Some(this) = weak.upgrade() {
                if let Some(handler_id) = handler_id_for_close.borrow_mut().take() {
                    this.imp().picture.get().disconnect(handler_id);
                }
                this.imp().fullscreen_preview_window.borrow_mut().take();
            }
            glib::Propagation::Proceed
        });
        window.connect_map(|window| {
            window.set_fullscreened(true);
            window.fullscreen();
        });

        window.present();
        window.set_fullscreened(true);
        window.fullscreen();
        let _ = glib::idle_add_local_once(move || {
            if let Some(p) = pic_for_fade.upgrade() {
                p.add_css_class("fade-shown");
            }
        });
        *self.imp().fullscreen_preview_window.borrow_mut() = Some(window);
    }

    fn update_fullscreen_button(&self) {
        let button = self.imp().fullscreen_btn.get();
        button.set_icon_name(super::VIEWER_FULLSCREEN_ICON);
        button.set_tooltip_text(Some(&tr("viewer.tooltip.fullscreen")));
    }
}

#[cfg(test)]
#[path = "fullscreen_window/tests.rs"]
mod tests;
