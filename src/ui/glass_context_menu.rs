use gtk4 as gtk;
use gtk4::gdk;
use gtk4::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

thread_local! {
    static OPEN_MENU: RefCell<Option<(gtk::glib::WeakRef<gtk::Overlay>, gtk::glib::WeakRef<gtk::Fixed>)>> =
        const { RefCell::new(None) };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GlassMenuItemKind {
    Normal,
    Suggested,
    Danger,
}

pub struct GlassMenuItem {
    label: String,
    kind: GlassMenuItemKind,
    on_activate: Rc<dyn Fn()>,
}

impl GlassMenuItem {
    pub fn new(
        label: impl Into<String>,
        kind: GlassMenuItemKind,
        on_activate: impl Fn() + 'static,
    ) -> Self {
        Self {
            label: label.into(),
            kind,
            on_activate: Rc::new(on_activate),
        }
    }
}

pub fn build_menu_panel_for_tests(items: Vec<GlassMenuItem>) -> gtk::Box {
    build_menu_panel(items, Rc::new(|| {}))
}

fn build_menu_panel(items: Vec<GlassMenuItem>, close: Rc<dyn Fn()>) -> gtk::Box {
    let panel = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(2)
        .css_classes(["glass-raised", "glass-context-menu"])
        .build();

    for item in items {
        let button = gtk::Button::builder()
            .label(item.label)
            .halign(gtk::Align::Fill)
            .css_classes(["glass-context-menu-item"])
            .build();
        match item.kind {
            GlassMenuItemKind::Normal => {}
            GlassMenuItemKind::Suggested => {
                button.add_css_class("glass-context-menu-item-suggested")
            }
            GlassMenuItemKind::Danger => button.add_css_class("glass-context-menu-item-danger"),
        }
        let on_activate = item.on_activate.clone();
        let close = close.clone();
        button.connect_clicked(move |_| {
            on_activate();
            close();
        });
        panel.append(&button);
    }

    panel
}

pub fn show(
    overlay: &gtk::Overlay,
    anchor: &gtk::Widget,
    anchor_x: f64,
    anchor_y: f64,
    items: Vec<GlassMenuItem>,
) {
    dismiss_open_menu();

    let layer = gtk::Fixed::builder()
        .hexpand(true)
        .vexpand(true)
        .halign(gtk::Align::Fill)
        .valign(gtk::Align::Fill)
        .can_focus(true)
        .css_classes(["glass-context-menu-layer"])
        .build();

    let overlay_weak = overlay.downgrade();
    let layer_weak = layer.downgrade();
    let close: Rc<dyn Fn()> = Rc::new(move || {
        let Some(overlay) = overlay_weak.upgrade() else {
            return;
        };
        let Some(layer) = layer_weak.upgrade() else {
            return;
        };
        close_menu_layer(&overlay, &layer);
    });

    let panel = build_menu_panel(items, close.clone());
    layer.put(&panel, 0.0, 0.0);
    let Some(point) = anchor.compute_point(
        overlay,
        &gtk::graphene::Point::new(anchor_x as f32, anchor_y as f32),
    ) else {
        return;
    };

    overlay.add_overlay(&layer);
    remember_open_menu(overlay, &layer);
    layer.grab_focus();

    let panel_weak = panel.downgrade();
    let click = gtk::GestureClick::new();
    click.set_button(0);
    click.connect_pressed(move |_, _, x, y| {
        if let Some(panel) = panel_weak.upgrade() {
            let alloc = panel.allocation();
            let inside_x = x >= alloc.x() as f64 && x <= (alloc.x() + alloc.width()) as f64;
            let inside_y = y >= alloc.y() as f64 && y <= (alloc.y() + alloc.height()) as f64;
            if inside_x && inside_y {
                return;
            }
        }
        close();
    });
    layer.add_controller(click);

    let overlay_for_key = overlay.clone();
    let layer_for_key = layer.clone();
    let key = gtk::EventControllerKey::new();
    key.connect_key_pressed(move |_, key, _, _| {
        if key == gdk::Key::Escape {
            overlay_for_key.remove_overlay(&layer_for_key);
            return gtk::glib::Propagation::Stop;
        }
        gtk::glib::Propagation::Proceed
    });
    layer.add_controller(key);

    let panel_min = panel.measure(gtk::Orientation::Horizontal, -1).1.max(128);
    let panel_height = panel
        .measure(gtk::Orientation::Vertical, panel_min)
        .1
        .max(1);
    let overlay_width = overlay.allocated_width().max(1);
    let overlay_height = overlay.allocated_height().max(1);
    let x = (point.x() as i32).clamp(0, (overlay_width - panel_min).max(0));
    let y = (point.y() as i32).clamp(0, (overlay_height - panel_height).max(0));
    layer.move_(&panel, x as f64, y as f64);
}

fn remember_open_menu(overlay: &gtk::Overlay, layer: &gtk::Fixed) {
    OPEN_MENU.with(|open| {
        *open.borrow_mut() = Some((overlay.downgrade(), layer.downgrade()));
    });
}

fn dismiss_open_menu() {
    let current = OPEN_MENU.with(|open| open.borrow_mut().take());
    let Some((overlay_weak, layer_weak)) = current else {
        return;
    };
    let Some(overlay) = overlay_weak.upgrade() else {
        return;
    };
    let Some(layer) = layer_weak.upgrade() else {
        return;
    };
    if layer.parent().is_some() {
        overlay.remove_overlay(&layer);
    }
}

fn close_menu_layer(overlay: &gtk::Overlay, layer: &gtk::Fixed) {
    if layer.parent().is_some() {
        overlay.remove_overlay(layer);
    }
    OPEN_MENU.with(|open| {
        let should_clear = open
            .borrow()
            .as_ref()
            .and_then(|(_, weak_layer)| weak_layer.upgrade())
            .as_ref()
            .is_some_and(|open_layer| open_layer == layer);
        if should_clear {
            *open.borrow_mut() = None;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gtk::test]
    fn opening_menu_on_new_overlay_dismisses_previous_overlay_menu() {
        let overlay_a = gtk::Overlay::new();
        let layer_a = gtk::Fixed::new();
        overlay_a.add_overlay(&layer_a);
        remember_open_menu(&overlay_a, &layer_a);

        let overlay_b = gtk::Overlay::new();
        let layer_b = gtk::Fixed::new();
        dismiss_open_menu();
        overlay_b.add_overlay(&layer_b);
        remember_open_menu(&overlay_b, &layer_b);

        assert!(
            layer_a.parent().is_none(),
            "opening another context menu should remove the previous one even on a different overlay"
        );
        assert!(
            layer_b.parent().is_some(),
            "new context menu layer should remain open"
        );

        dismiss_open_menu();
        assert!(layer_b.parent().is_none());
    }
}
