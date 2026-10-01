use super::ViewerPage;
use gtk4::prelude::*;
use gtk4::subclass::prelude::ObjectSubclassIsExt;

impl ViewerPage {
    pub(super) fn setup_zoom_controls(&self) {
        let imp = self.imp();

        let weak = self.downgrade();
        imp.zoom_in_btn.get().connect_clicked(move |_| {
            if let Some(this) = weak.upgrade() {
                this.step_viewer_zoom(1);
            }
        });

        let weak = self.downgrade();
        imp.zoom_out_btn.get().connect_clicked(move |_| {
            if let Some(this) = weak.upgrade() {
                this.step_viewer_zoom(-1);
            }
        });

        let weak = self.downgrade();
        imp.zoom_reset_btn.get().connect_clicked(move |_| {
            if let Some(this) = weak.upgrade() {
                this.reset_viewer_zoom();
            }
        });

        let weak = self.downgrade();
        imp.rotate_left_btn.get().connect_clicked(move |_| {
            if let Some(this) = weak.upgrade() {
                this.rotate_viewer_image(-90);
            }
        });

        let weak = self.downgrade();
        imp.rotate_right_btn.get().connect_clicked(move |_| {
            if let Some(this) = weak.upgrade() {
                this.rotate_viewer_image(90);
            }
        });

        self.update_zoom_buttons();
    }

    pub(super) fn step_viewer_zoom(&self, direction: i32) {
        let next = step_zoom(self.imp().zoom_scale.get(), direction);
        self.set_viewer_zoom(
            next,
            self.imp().zoom_pan_x.get(),
            self.imp().zoom_pan_y.get(),
        );
    }

    pub(super) fn reset_viewer_zoom(&self) {
        self.set_viewer_zoom(super::MIN_VIEWER_ZOOM, 0.0, 0.0);
    }

    pub(super) fn reset_viewer_transform(&self) {
        self.imp().viewer_rotation_degrees.set(0);
        self.imp().zoom_wheel_remainder.set(0.0);
        self.imp().pan_gesture_start.set(None);
        self.set_viewer_zoom(super::MIN_VIEWER_ZOOM, 0.0, 0.0);
    }

    pub(super) fn rotate_viewer_image(&self, delta_degrees: i32) {
        let current = self.imp().viewer_rotation_degrees.get();
        let next = (current + delta_degrees).rem_euclid(360);
        self.imp().viewer_rotation_degrees.set(next);
        self.set_viewer_zoom(
            self.imp().zoom_scale.get(),
            self.imp().zoom_pan_x.get(),
            self.imp().zoom_pan_y.get(),
        );
    }

    pub(super) fn set_viewer_zoom(&self, scale: f64, pan_x: f64, pan_y: f64) {
        let picture = self.imp().picture.get();
        let scale = scale.clamp(super::MIN_VIEWER_ZOOM, super::MAX_VIEWER_ZOOM);
        let rotation = self.imp().viewer_rotation_degrees.get();
        let (image_width, image_height) = picture
            .paintable()
            .map(|paintable| {
                (
                    paintable.intrinsic_width().max(0) as f64,
                    paintable.intrinsic_height().max(0) as f64,
                )
            })
            .unwrap_or((0.0, 0.0));
        let (pan_x, pan_y) = clamp_zoom_pan(
            scale,
            pan_x,
            pan_y,
            picture.allocated_width() as f64,
            picture.allocated_height() as f64,
            image_width,
            image_height,
            rotation,
        );
        self.imp().zoom_scale.set(scale);
        self.imp().zoom_pan_x.set(pan_x);
        self.imp().zoom_pan_y.set(pan_y);
        self.update_zoom_transform();
        self.update_zoom_buttons();
    }

    #[cfg(test)]
    pub(super) fn set_viewer_zoom_for_tests(&self, scale: f64, pan_x: f64, pan_y: f64) {
        self.set_viewer_zoom(scale, pan_x, pan_y);
    }

    fn update_zoom_transform(&self) {
        let imp = self.imp();
        let scale = imp.zoom_scale.get();
        let pan_x = imp.zoom_pan_x.get();
        let pan_y = imp.zoom_pan_y.get();
        let rotation = imp.viewer_rotation_degrees.get();
        if let Some(provider) = imp.zoom_provider.borrow().as_ref() {
            provider.load_from_data(&format!(
                "picture.viewer-image-frame {{ transform: translate({pan_x}px, {pan_y}px) rotate({rotation}deg) scale({scale}); }}"
            ));
        }
        imp.picture.get().queue_draw();
    }

    pub(super) fn set_zoom_controls_visible(&self, visible: bool) {
        if let Some(parent) = self.imp().zoom_in_btn.get().parent() {
            parent.set_visible(visible);
        }
        self.update_zoom_buttons();
    }

    fn update_zoom_buttons(&self) {
        let imp = self.imp();
        let zoomed = imp.zoom_scale.get() > super::MIN_VIEWER_ZOOM;
        imp.zoom_in_btn.get().set_visible(true);
        imp.zoom_out_btn.get().set_visible(zoomed);
        imp.zoom_reset_btn.get().set_visible(zoomed);
        imp.rotate_left_btn.get().set_visible(!zoomed);
        imp.rotate_right_btn.get().set_visible(!zoomed);
    }
}

pub(super) fn step_zoom(current: f64, direction: i32) -> f64 {
    let factor = if direction >= 0 {
        super::VIEWER_ZOOM_STEP
    } else {
        1.0 / super::VIEWER_ZOOM_STEP
    };
    (current * factor).clamp(super::MIN_VIEWER_ZOOM, super::MAX_VIEWER_ZOOM)
}

pub(super) fn clamp_zoom_pan(
    scale: f64,
    pan_x: f64,
    pan_y: f64,
    viewport_width: f64,
    viewport_height: f64,
    image_width: f64,
    image_height: f64,
    rotation: i32,
) -> (f64, f64) {
    if scale <= super::MIN_VIEWER_ZOOM
        || viewport_width <= 0.0
        || viewport_height <= 0.0
        || image_width <= 0.0
        || image_height <= 0.0
    {
        return (0.0, 0.0);
    }

    // GtkPicture lays out the *unrotated* paintable; the CSS transform then
    // rotates that rendered box, so the contain scale comes from the source
    // aspect ratio while the pan bounds come from the rotated bounding box.
    let fit = (viewport_width / image_width).min(viewport_height / image_height);
    let (visible_width, visible_height) = match rotation.rem_euclid(360) {
        90 | 270 => (image_height * fit, image_width * fit),
        _ => (image_width * fit, image_height * fit),
    };
    let max_x = (visible_width * scale - viewport_width).max(0.0) / 2.0;
    let max_y = (visible_height * scale - viewport_height).max(0.0) / 2.0;
    (pan_x.clamp(-max_x, max_x), pan_y.clamp(-max_y, max_y))
}

#[cfg(test)]
#[path = "transform/tests.rs"]
mod tests;
