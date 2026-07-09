use gtk4 as gtk;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::subclass::prelude::ObjectSubclassIsExt;

use super::{CropOverlayUpdate, ViewerPage};

const CROP_HANDLE_RADIUS: f64 = 14.0;
const CROP_MIN_SOURCE_SIZE: u32 = 24;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct ImageRect {
    pub(super) x: f64,
    pub(super) y: f64,
    pub(super) width: f64,
    pub(super) height: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CropDragMode {
    Move,
    ResizeNw,
    ResizeNe,
    ResizeSw,
    ResizeSe,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct CropDragState {
    pub(super) mode: CropDragMode,
    pub(super) rect: (u32, u32, u32, u32),
}

impl ViewerPage {
    pub(super) fn set_crop_overlay(&self, update: CropOverlayUpdate) {
        let imp = self.imp();
        imp.crop_overlay_active.set(update.active);
        if !update.active {
            imp.crop_overlay_selected.set(false);
        }
        imp.crop_overlay_rect.set(update.rect);
        imp.crop_overlay_dimensions.set(update.image_dimensions);
        if !update.active {
            imp.crop_drag.borrow_mut().take();
        }
        imp.crop_overlay.get().set_visible(update.active);
        imp.crop_overlay.get().queue_draw();
    }

    pub(super) fn setup_crop_overlay(&self) {
        let overlay = self.imp().crop_overlay.get();
        overlay.set_draw_func(
            glib::clone!(@weak self as this => move |area, cr, width, height| {
                this.draw_crop_overlay(area, cr, width, height);
            }),
        );

        let drag = gtk::GestureDrag::new();
        drag.connect_drag_begin(glib::clone!(@weak self as this => move |_, x, y| {
            this.begin_crop_drag(x, y);
        }));
        drag.connect_drag_update(glib::clone!(@weak self as this => move |_, dx, dy| {
            this.update_crop_drag(dx, dy);
        }));
        drag.connect_drag_end(glib::clone!(@weak self as this => move |_, _, _| {
            this.imp().crop_drag.borrow_mut().take();
            this.imp().crop_overlay_selected.set(false);
            this.imp().crop_overlay.get().queue_draw();
        }));
        overlay.add_controller(drag);
    }

    fn draw_crop_overlay(
        &self,
        _area: &gtk::DrawingArea,
        cr: &gtk::cairo::Context,
        width: i32,
        height: i32,
    ) {
        let imp = self.imp();
        if !imp.crop_overlay_active.get() {
            return;
        }
        let Some(rect) = imp.crop_overlay_rect.get() else {
            return;
        };
        let image_dimensions = imp.crop_overlay_dimensions.get();
        let Some(image_rect) =
            compute_contained_image_rect(width as f64, height as f64, image_dimensions)
        else {
            return;
        };
        let Some(widget_rect) = crop_rect_to_widget(rect, image_dimensions, image_rect) else {
            return;
        };

        cr.set_source_rgba(0.0, 0.0, 0.0, 0.42);
        cr.rectangle(0.0, 0.0, width as f64, height as f64);
        cr.rectangle(
            widget_rect.x,
            widget_rect.y,
            widget_rect.width,
            widget_rect.height,
        );
        cr.set_fill_rule(gtk::cairo::FillRule::EvenOdd);
        let _ = cr.fill();
        cr.set_fill_rule(gtk::cairo::FillRule::Winding);

        let selected = imp.crop_overlay_selected.get();
        if selected {
            cr.set_source_rgba(0.38, 0.72, 1.0, 0.98);
            cr.set_line_width(3.0);
        } else {
            cr.set_source_rgba(1.0, 1.0, 1.0, 0.92);
            cr.set_line_width(2.0);
        }
        cr.rectangle(
            widget_rect.x,
            widget_rect.y,
            widget_rect.width,
            widget_rect.height,
        );
        let _ = cr.stroke();

        for (x, y) in crop_handle_points(widget_rect) {
            let radius = if selected { 7.0 } else { 5.0 };
            cr.arc(x, y, radius, 0.0, std::f64::consts::TAU);
            let _ = cr.fill();
        }
    }

    fn begin_crop_drag(&self, x: f64, y: f64) {
        let imp = self.imp();
        if !imp.crop_overlay_active.get() {
            return;
        }
        let Some(rect) = imp.crop_overlay_rect.get() else {
            return;
        };
        let image_dimensions = imp.crop_overlay_dimensions.get();
        let overlay = imp.crop_overlay.get();
        let Some(image_rect) = compute_contained_image_rect(
            overlay.allocated_width() as f64,
            overlay.allocated_height() as f64,
            image_dimensions,
        ) else {
            return;
        };
        let Some(widget_rect) = crop_rect_to_widget(rect, image_dimensions, image_rect) else {
            return;
        };
        let Some(mode) = hit_crop_drag_mode(x, y, widget_rect) else {
            imp.crop_overlay_selected.set(false);
            imp.crop_overlay.get().queue_draw();
            return;
        };
        imp.crop_overlay_selected.set(true);
        imp.crop_overlay.get().queue_draw();
        *imp.crop_drag.borrow_mut() = Some(CropDragState { mode, rect });
    }

    fn update_crop_drag(&self, dx: f64, dy: f64) {
        let Some(drag) = *self.imp().crop_drag.borrow() else {
            return;
        };
        let image_dimensions = self.imp().crop_overlay_dimensions.get();
        let overlay = self.imp().crop_overlay.get();
        let Some(image_rect) = compute_contained_image_rect(
            overlay.allocated_width() as f64,
            overlay.allocated_height() as f64,
            image_dimensions,
        ) else {
            return;
        };
        let sx = dx / image_rect.width * image_dimensions.0 as f64;
        let sy = dy / image_rect.height * image_dimensions.1 as f64;
        let rect = drag_rect(drag, sx, sy, image_dimensions);
        self.imp().crop_overlay_rect.set(Some(rect));
        self.imp().crop_overlay.get().queue_draw();
        self.imp()
            .editor_panel
            .get()
            .set_crop_rect_from_overlay(rect);
    }
}

pub(super) fn compute_contained_image_rect(
    widget_width: f64,
    widget_height: f64,
    image_dimensions: (u32, u32),
) -> Option<ImageRect> {
    let (image_width, image_height) = image_dimensions;
    if widget_width <= 0.0 || widget_height <= 0.0 || image_width == 0 || image_height == 0 {
        return None;
    }
    let widget_ratio = widget_width / widget_height;
    let image_ratio = image_width as f64 / image_height as f64;
    let (width, height) = if widget_ratio > image_ratio {
        let height = widget_height;
        (height * image_ratio, height)
    } else {
        let width = widget_width;
        (width, width / image_ratio)
    };
    Some(ImageRect {
        x: (widget_width - width) / 2.0,
        y: (widget_height - height) / 2.0,
        width,
        height,
    })
}

pub(super) fn crop_rect_to_widget(
    rect: (u32, u32, u32, u32),
    image_dimensions: (u32, u32),
    image_rect: ImageRect,
) -> Option<ImageRect> {
    let (image_width, image_height) = image_dimensions;
    if image_width == 0 || image_height == 0 {
        return None;
    }
    Some(ImageRect {
        x: image_rect.x + rect.0 as f64 / image_width as f64 * image_rect.width,
        y: image_rect.y + rect.1 as f64 / image_height as f64 * image_rect.height,
        width: rect.2 as f64 / image_width as f64 * image_rect.width,
        height: rect.3 as f64 / image_height as f64 * image_rect.height,
    })
}

fn crop_handle_points(rect: ImageRect) -> [(f64, f64); 4] {
    [
        (rect.x, rect.y),
        (rect.x + rect.width, rect.y),
        (rect.x, rect.y + rect.height),
        (rect.x + rect.width, rect.y + rect.height),
    ]
}

pub(super) fn hit_crop_drag_mode(x: f64, y: f64, rect: ImageRect) -> Option<CropDragMode> {
    for (idx, (hx, hy)) in crop_handle_points(rect).into_iter().enumerate() {
        if (x - hx).hypot(y - hy) <= CROP_HANDLE_RADIUS {
            return Some(match idx {
                0 => CropDragMode::ResizeNw,
                1 => CropDragMode::ResizeNe,
                2 => CropDragMode::ResizeSw,
                _ => CropDragMode::ResizeSe,
            });
        }
    }
    let inside =
        x >= rect.x && x <= rect.x + rect.width && y >= rect.y && y <= rect.y + rect.height;
    inside.then_some(CropDragMode::Move)
}

pub(super) fn drag_rect(
    drag: CropDragState,
    dx: f64,
    dy: f64,
    image_dimensions: (u32, u32),
) -> (u32, u32, u32, u32) {
    let (image_width, image_height) = image_dimensions;
    let (x, y, width, height) = drag.rect;
    let min = CROP_MIN_SOURCE_SIZE
        .min(image_width)
        .min(image_height)
        .max(1);
    match drag.mode {
        CropDragMode::Move => {
            let nx = (x as f64 + dx)
                .round()
                .clamp(0.0, image_width.saturating_sub(width) as f64) as u32;
            let ny = (y as f64 + dy)
                .round()
                .clamp(0.0, image_height.saturating_sub(height) as f64) as u32;
            (nx, ny, width, height)
        }
        CropDragMode::ResizeNw => resize_from_edges(
            (x as f64 + dx).round() as i32,
            (y as f64 + dy).round() as i32,
            (x + width) as i32,
            (y + height) as i32,
            image_dimensions,
            min,
        ),
        CropDragMode::ResizeNe => resize_from_edges(
            x as i32,
            (y as f64 + dy).round() as i32,
            (x as f64 + width as f64 + dx).round() as i32,
            (y + height) as i32,
            image_dimensions,
            min,
        ),
        CropDragMode::ResizeSw => resize_from_edges(
            (x as f64 + dx).round() as i32,
            y as i32,
            (x + width) as i32,
            (y as f64 + height as f64 + dy).round() as i32,
            image_dimensions,
            min,
        ),
        CropDragMode::ResizeSe => resize_from_edges(
            x as i32,
            y as i32,
            (x as f64 + width as f64 + dx).round() as i32,
            (y as f64 + height as f64 + dy).round() as i32,
            image_dimensions,
            min,
        ),
    }
}

pub(super) fn resize_from_edges(
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
    image_dimensions: (u32, u32),
    min_size: u32,
) -> (u32, u32, u32, u32) {
    let (image_width, image_height) = image_dimensions;
    let left = left.clamp(0, image_width.saturating_sub(min_size) as i32);
    let top = top.clamp(0, image_height.saturating_sub(min_size) as i32);
    let right = right.clamp(left + min_size as i32, image_width as i32);
    let bottom = bottom.clamp(top + min_size as i32, image_height as i32);
    (
        left as u32,
        top as u32,
        (right - left) as u32,
        (bottom - top) as u32,
    )
}
