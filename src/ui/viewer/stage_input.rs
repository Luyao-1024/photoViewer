use super::transform::step_zoom;
use super::{ViewerPage, MIN_VIEWER_ZOOM};
use gtk4 as gtk;
use gtk4::gdk;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::subclass::prelude::ObjectSubclassIsExt;

pub(super) fn zoom_for_wheel_delta(
    current: f64,
    delta_y: f64,
    remainder: f64,
    steps_per_degree: f64,
) -> (f64, f64) {
    let accumulated = remainder - delta_y;
    let direction = (accumulated / steps_per_degree).floor();
    (
        step_zoom(current, direction as i32),
        accumulated - direction * steps_per_degree,
    )
}

pub(super) fn should_pan_from_drag(scale: f64) -> bool {
    scale > MIN_VIEWER_ZOOM
}

pub(super) fn panned_from(
    start_pan_x: f64,
    start_pan_y: f64,
    delta_x: f64,
    delta_y: f64,
) -> (f64, f64) {
    (start_pan_x + delta_x, start_pan_y + delta_y)
}

impl ViewerPage {
    pub(super) fn setup_image_stage_input(&self) {
        let overlay = self.imp().image_overlay.get();

        let scroll = gtk::EventControllerScroll::builder()
            .flags(gtk::EventControllerScrollFlags::VERTICAL)
            .build();
        scroll.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = self.downgrade();
        scroll.connect_scroll(move |controller, _delta_x, delta_y| {
            let Some(this) = weak.upgrade() else {
                return glib::Propagation::Proceed;
            };
            let modifiers = controller.current_event_state();
            if !modifiers.contains(gdk::ModifierType::CONTROL_MASK)
                || !image_stage_input_active(&this)
                || delta_y == 0.0
            {
                return glib::Propagation::Proceed;
            }
            let (next, remainder) = zoom_for_wheel_delta(
                this.imp().zoom_scale.get(),
                delta_y,
                this.imp().zoom_wheel_remainder.get(),
                super::VIEWER_ZOOM_WHEEL_STEPS_PER_DEGREE,
            );
            this.imp().zoom_wheel_remainder.set(remainder);
            this.set_viewer_zoom_for_input(
                next,
                this.imp().zoom_pan_x.get(),
                this.imp().zoom_pan_y.get(),
            );
            glib::Propagation::Stop
        });
        overlay.add_controller(scroll);

        let drag = gtk::GestureDrag::new();
        drag.set_property("button", gdk::BUTTON_PRIMARY);
        let weak = self.downgrade();
        drag.connect_drag_begin(move |_, x, y| {
            let Some(this) = weak.upgrade() else {
                return;
            };
            let current = (
                x,
                y,
                this.imp().zoom_pan_x.get(),
                this.imp().zoom_pan_y.get(),
            );
            this.imp().pan_gesture_start.set(None);
            if !image_stage_input_active(&this)
                || !should_pan_from_drag(this.imp().zoom_scale.get())
            {
                return;
            }
            this.imp().pan_gesture_start.set(Some(current));
        });
        let weak = self.downgrade();
        drag.connect_drag_update(move |_, delta_x, delta_y| {
            let Some(this) = weak.upgrade() else {
                return;
            };
            let Some((_, _, pan_x, pan_y)) = this.imp().pan_gesture_start.get() else {
                return;
            };
            if !should_pan_from_drag(this.imp().zoom_scale.get()) {
                return;
            }
            let next = panned_from(pan_x, pan_y, delta_x, delta_y);
            this.set_viewer_zoom_for_input(this.imp().zoom_scale.get(), next.0, next.1);
        });
        let weak = self.downgrade();
        drag.connect_drag_end(move |_, _, _| {
            if let Some(this) = weak.upgrade() {
                this.imp().pan_gesture_start.set(None);
            }
        });
        overlay.add_controller(drag);
    }

    pub(super) fn set_viewer_zoom_for_input(&self, scale: f64, pan_x: f64, pan_y: f64) {
        if !image_stage_input_active(self) {
            return;
        }
        let picture = self.imp().picture.get();
        if picture.width() <= 0 || picture.height() <= 0 {
            return;
        }
        self.set_viewer_zoom(scale, pan_x, pan_y);
    }
}

fn image_stage_input_active(viewer: &ViewerPage) -> bool {
    let imp = viewer.imp();
    imp.picture.get().is_visible() && !viewer.is_editing_keyboard_scope()
}

#[cfg(test)]
#[path = "stage_input/tests.rs"]
mod tests;
