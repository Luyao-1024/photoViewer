//! Exercise GTK's resolved colors, not just the CSS source. Optionally export
//! the same fixture with PHOTOVIEWER_GLASS_SCREENSHOTS=<directory>.
use super::*;
use libadwaita as adw;
use std::rc::Rc;

fn luminance(color: &gdk::RGBA) -> f64 {
    let linear = |v: f32| {
        let v = f64::from(v);
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * linear(color.red()) + 0.7152 * linear(color.green()) + 0.0722 * linear(color.blue())
}

fn settle(milliseconds: u64) {
    glib::MainContext::default().block_on(glib::timeout_future(std::time::Duration::from_millis(
        milliseconds,
    )));
}

fn pixels(window: &gtk::Window, widget: &impl IsA<gtk::Widget>) -> (usize, Vec<u8>) {
    let widget = widget.as_ref();
    let snapshot = gtk::Snapshot::new();
    gtk::WidgetPaintable::new(Some(widget)).snapshot(
        &snapshot,
        widget.width() as f64,
        widget.height() as f64,
    );
    let texture = window.renderer().unwrap().render_texture(
        snapshot.to_node().expect("mapped widget must render"),
        Some(&gtk::graphene::Rect::new(
            0.0,
            0.0,
            widget.width() as f32,
            widget.height() as f32,
        )),
    );
    let width = texture.width() as usize;
    let mut data = vec![0; width * texture.height() as usize * 4];
    texture.download(&mut data, width * 4);
    (width, data)
}

#[gtk::test]
fn selector_contrast_interpolates_in_both_material_modes() {
    adw::init().unwrap();
    let display = gdk::Display::default().unwrap();
    let provider = gtk::CssProvider::new();
    gtk::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
    let selector = crate::ui::mode_selector::ModeSelector::new();
    let label = selector
        .first_child()
        .unwrap()
        .first_child()
        .unwrap()
        .first_child()
        .unwrap();
    let dot = selector.last_child().unwrap().first_child().unwrap();
    let window = gtk::Window::builder().child(&selector).build();
    window.present();
    for liquid in [true, false] {
        provider.load_from_data(&runtime_compatible_css(&build_css_with_transparency(
            liquid, 0.5,
        )));
        selector.set_light_background(false);
        settle(500);
        let (width, dark_pixels) = pixels(&window, &selector);
        // A clear part of the capsule, away from text, border and shadow.
        let sample = (8 * width + width / 2) * 4;
        selector.set_light_background(true);
        settle(100);
        for widget in [selector.upcast_ref::<gtk::Widget>(), &label, &dot] {
            let red = widget.style_context().color().red();
            assert!(
                red > 0.15 && red < 0.98,
                "{} contrast must interpolate: {red}",
                widget.type_().name()
            );
        }
        let (_, intermediate) = pixels(&window, &selector);
        settle(400);
        let (_, light_pixels) = pixels(&window, &selector);
        assert!(intermediate[sample] > dark_pixels[sample] + 2);
        assert!(intermediate[sample] + 2 < light_pixels[sample]);
        assert!(label.style_context().color().red() < 0.1);
        selector.set_light_background(false);
        settle(100);
        let before_reverse = label.style_context().color().red();
        assert!(before_reverse > 0.15 && before_reverse < 0.98);
        selector.set_light_background(true);
        let after_reverse = label.style_context().color().red();
        assert!(
            (after_reverse - before_reverse).abs() < 0.08,
            "reversal must not jump"
        );
        settle(500);
        assert!(label.style_context().color().red() < 0.1);
    }
    window.close();
    gtk::style_context_remove_provider_for_display(&display, &provider);
}

#[gtk::test]
fn thumbnail_emphasis_covers_white_image_edges() {
    adw::init().unwrap();
    let display = gdk::Display::default().unwrap();
    let provider = gtk::CssProvider::new();
    gtk::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
    let tile = crate::ui::square_tile::SquareTile::new();
    let white = gdk::MemoryTexture::new(
        32,
        32,
        gdk::MemoryFormat::R8g8b8a8,
        &glib::Bytes::from_owned(vec![255u8; 32 * 32 * 4]),
        32 * 4,
    );
    tile.set_paintable(Some(&white));
    let window = gtk::Window::builder()
        .default_width(180)
        .default_height(180)
        .child(&tile)
        .build();
    window.present();
    for liquid in [true, false] {
        provider.load_from_data(&runtime_compatible_css(&build_css_with_transparency(
            liquid, 0.5,
        )));
        settle(300);
        let (width, normal) = pixels(&window, &tile);
        let height = normal.len() / (width * 4);
        for class in ["thumb-pointer-hover", "media-selected"] {
            tile.add_css_class(class);
            settle(250);
            let (active_width, active) = pixels(&window, &tile);
            assert_eq!(active_width, width, "emphasis must not resize the tile");
            assert_eq!(active.len(), normal.len());
            // Follow the actual white picture extent, excluding rounded corners.
            for (x, y) in (0..width)
                .map(|x| (x, height / 2))
                .chain((0..height).map(|y| (width / 2, y)))
            {
                let i = (y * width + x) * 4;
                if normal[i] > 245 && normal[i + 3] > 245 {
                    assert!(
                        active[i] < 210,
                        "uncovered white pixel at ({x}, {y}) in {class}: {}",
                        active[i]
                    );
                }
            }
            tile.remove_css_class(class);
            settle(250);
        }
    }
    window.close();
    gtk::style_context_remove_provider_for_display(&display, &provider);
}

#[gtk::test]
fn materials_resolve_in_both_themes_and_at_transparency_endpoints() {
    adw::init().unwrap();
    let manager = adw::StyleManager::default();
    let previous_scheme = manager.color_scheme();
    let display = gdk::Display::default().unwrap();
    let provider = gtk::CssProvider::new();
    let errors = Rc::new(RefCell::new(Vec::new()));
    let recorded = errors.clone();
    provider
        .connect_parsing_error(move |_, _, error| recorded.borrow_mut().push(error.to_string()));
    gtk::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );

    let content = gtk::Box::new(gtk::Orientation::Vertical, 24);
    content.set_margin_top(32);
    content.set_margin_bottom(32);
    content.set_margin_start(32);
    content.set_margin_end(32);
    let title = gtk::Label::new(Some("Liquid Glass · 材质检查"));
    title.add_css_class("title-1");
    content.append(&title);
    let selectors = gtk::Box::new(gtk::Orientation::Horizontal, 24);
    let dark = crate::ui::mode_selector::ModeSelector::new();
    let light = crate::ui::mode_selector::ModeSelector::new();
    light.set_light_background(true);
    selectors.append(&dark);
    selectors.append(&light);
    content.append(&selectors);
    let panel = gtk::Box::new(gtk::Orientation::Vertical, 12);
    panel.add_css_class("viewer-floating-panel");
    let heading = gtk::Label::new(Some("照片详情 / Photo details"));
    heading.set_margin_top(16);
    panel.append(&heading);
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    buttons.set_margin_start(16);
    buttons.set_margin_end(16);
    buttons.set_margin_bottom(16);
    let mut semantic_buttons = Vec::new();
    for (label, class) in [
        ("取消", ""),
        ("保存副本", "glass-toolbar-suggested"),
        ("删除", "glass-toolbar-danger"),
    ] {
        let button = gtk::Button::with_label(label);
        button.add_css_class("glass-toolbar-button");
        if !class.is_empty() {
            button.add_css_class(class);
        }
        buttons.append(&button);
        semantic_buttons.push(button);
    }
    panel.append(&buttons);
    content.append(&panel);
    let window = gtk::Window::builder()
        .default_width(640)
        .default_height(400)
        .child(&content)
        .build();
    window.present();

    for (theme, scheme) in [
        ("light", adw::ColorScheme::ForceLight),
        ("dark", adw::ColorScheme::ForceDark),
    ] {
        manager.set_color_scheme(scheme);
        for liquid in [true, false] {
            for transparency in [0.0, 0.5, 1.0] {
                provider.load_from_data(&runtime_compatible_css(&build_css_with_transparency(
                    liquid,
                    transparency,
                )));
                assert!(
                    errors.borrow().is_empty(),
                    "GTK CSS errors: {:?}",
                    errors.borrow()
                );
                glib::MainContext::default()
                    .block_on(glib::timeout_future(std::time::Duration::from_millis(400)));
                assert!(luminance(&dark.style_context().color()) > 0.9);
                assert!(luminance(&light.style_context().color()) < 0.03);
                let reading = panel
                    .style_context()
                    .lookup_color("glass_reading_bg")
                    .unwrap();
                assert!(
                    reading.alpha() >= 0.719,
                    "reading fill lost at {transparency}"
                );
                let expected = panel
                    .style_context()
                    .lookup_color("window_bg_color")
                    .unwrap();
                assert!((reading.red() - expected.red()).abs() < 0.01);
                for (button, role) in semantic_buttons[1..]
                    .iter()
                    .zip(["accent_color", "error_color"])
                {
                    let actual = button.style_context().color();
                    let expected = button.style_context().lookup_color(role).unwrap();
                    assert!(
                        (actual.red() - expected.red()).abs() < 0.01,
                        "{role} must follow {theme}"
                    );
                    assert!((actual.green() - expected.green()).abs() < 0.01);
                    assert!((actual.blue() - expected.blue()).abs() < 0.01);
                }
                if let Some(directory) = std::env::var_os("PHOTOVIEWER_GLASS_SCREENSHOTS") {
                    let directory = std::path::PathBuf::from(directory);
                    std::fs::create_dir_all(&directory).unwrap();
                    let snapshot = gtk::Snapshot::new();
                    gtk::WidgetPaintable::new(Some(&window)).snapshot(
                        &snapshot,
                        window.width() as f64,
                        window.height() as f64,
                    );
                    let node = snapshot.to_node().expect("mapped window must render");
                    let texture = window.renderer().unwrap().render_texture(&node, None);
                    texture
                        .save_to_png(directory.join(format!(
                            "{theme}-{}-{}.png",
                            if liquid { "liquid" } else { "plain" },
                            (transparency * 100.0) as u32
                        )))
                        .unwrap();
                }
            }
        }
    }
    window.close();
    gtk::style_context_remove_provider_for_display(&display, &provider);
    manager.set_color_scheme(previous_scheme);
}

/// The Photos grid is where Space (select) and Delete (move to trash) act on
/// whatever tile holds keyboard focus, so that ring has to render, not merely
/// exist in the CSS source. GtkGridView owns focus on its internal list-item
/// wrapper, which is the node path exercised here against a white thumbnail.
///
/// The check is deliberately colour-agnostic: `@accent_bg_color` is the theme's
/// accent, which the desktop user can change, so the test compares the focused
/// snapshot against the unfocused one and requires the difference to hug the
/// tile edge. That also proves the ring repaints nothing over the picture.
#[gtk::test]
fn virtual_grid_tile_focus_ring_renders() {
    adw::init().unwrap();
    let display = gdk::Display::default().unwrap();
    let provider = gtk::CssProvider::new();
    gtk::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );

    let tiles: Rc<RefCell<Vec<crate::ui::square_tile::SquareTile>>> = Rc::default();
    let factory = gtk::SignalListItemFactory::new();
    {
        let tiles = tiles.clone();
        factory.connect_setup(move |_, object| {
            let Ok(list_item) = object.clone().downcast::<gtk::ListItem>() else {
                return;
            };
            let tile = crate::ui::square_tile::SquareTile::new();
            tile.set_paintable(Some(&gdk::MemoryTexture::new(
                32,
                32,
                gdk::MemoryFormat::R8g8b8a8,
                &glib::Bytes::from_owned(vec![255u8; 32 * 32 * 4]),
                32 * 4,
            )));
            tile.set_can_focus(true);
            tiles.borrow_mut().push(tile.clone());
            list_item.set_child(Some(&tile));
        });
    }

    let model = gtk::NoSelection::new(Some(gtk::StringList::new(&["a", "b", "c", "d"])));
    let grid = gtk::GridView::builder()
        .model(&model)
        .factory(&factory)
        .build();
    grid.add_css_class("virtual-media-grid-view");
    grid.set_max_columns(2);
    let window = gtk::Window::builder()
        .default_width(260)
        .default_height(260)
        .child(&grid)
        .build();
    window.present();
    settle(300);
    assert!(
        tiles.borrow().iter().any(|t| t.width() > 20),
        "the GridView fixture must allocate real tiles"
    );

    let tile = tiles.borrow()[0].clone();
    let wrapper = tile
        .parent()
        .expect("tile must sit in a GridView child")
        .upcast::<gtk::Widget>();
    assert_eq!(
        wrapper.css_name().as_str(),
        "child",
        "GtkGridView must keep its list-item node named `child` for the CSS selector"
    );

    for liquid in [true, false] {
        provider.load_from_data(&runtime_compatible_css(&build_css_with_transparency(
            liquid, 0.0,
        )));
        settle(300);

        // GtkGridView owns keyboard focus on its internal list-item wrapper and
        // rejects the tile as the focus widget, so this is the reachable node
        // path for arrow navigation and grab_visible_focus. The tile's own
        // `:focus` selector stays in the CSS for the direct `grab_focus()`
        // fallback in glass_context_menu.rs; it cannot be provoked here.
        // The snapshot is taken on the wrapper because that is what the user
        // sees: the accent ring is drawn inside the card and the dark hairline
        // on the wrapper node itself.
        gtk::prelude::GtkWindowExt::set_focus(&window, None::<&gtk::Widget>);
        settle(250);
        let (width, unfocused) = pixels(&window, &wrapper);
        let height = unfocused.len() / (width * 4);

        gtk::prelude::GtkWindowExt::set_focus(&window, Some(&wrapper));
        assert_eq!(
            gtk::prelude::GtkWindowExt::focus(&window)
                .as_ref()
                .map(gtk::Widget::as_ptr),
            Some(wrapper.as_ptr()),
            "the fixture must be able to focus the GridView list item ({liquid} mode)"
        );
        settle(250);
        let (_, focused) = pixels(&window, &wrapper);

        assert_eq!(
            focused.len(),
            unfocused.len(),
            "focus must not resize the list item"
        );
        let changed = |x: usize, y: usize| -> bool {
            let i = (y * width + x) * 4;
            (0..3).any(|c| (i32::from(focused[i + c]) - i32::from(unfocused[i + c])).abs() > 24)
        };
        let repaint = (0..height)
            .flat_map(|y| (0..width).map(move |x| changed(x, y)))
            .filter(|c| *c)
            .count();
        // A 3 px band around the edge of the tile repaints roughly this much.
        assert!(
            repaint >= 2 * (width + height),
            "focus must paint a ring around the photo, repainted {repaint} px \
             in a {width}x{height} tile ({liquid} mode)"
        );
        let inner = (8..height.saturating_sub(8))
            .any(|y| (8..width.saturating_sub(8)).any(|x| changed(x, y)));
        assert!(
            !inner,
            "focus must only ring the tile edge, not repaint the picture ({liquid} mode)"
        );

        // Contrast is checked without naming the accent colour, which the
        // desktop user can change: the ring must separate widely from the white
        // photo, and the hairline behind it must reach near-black.
        let row = height / 2;
        let band = (0..width).filter(|x| *x < 4 || *x >= width - 4);
        let separation = band
            .clone()
            .map(|x| {
                let i = (row * width + x) * 4;
                (0..3)
                    .map(|c| (i32::from(unfocused[i + c]) - i32::from(focused[i + c])).abs())
                    .max()
                    .unwrap_or(0)
            })
            .max()
            .unwrap_or(0);
        assert!(
            separation >= 60,
            "the focus ring must be clearly distinguishable from a white photo, \
             closest band colour was only {separation} apart ({liquid} mode)"
        );
        let dark_halo = (0..4.min(width)).any(|x| {
            let i = (row * width + x) * 4;
            (0..3).all(|c| focused[i + c] < 180 && unfocused[i + c] > 240)
        });
        assert!(
            dark_halo,
            "a dark hairline must back the accent ring so it survives a photo that \
             matches the accent ({liquid} mode)"
        );
    }

    window.close();
    gtk::style_context_remove_provider_for_display(&display, &provider);
}
