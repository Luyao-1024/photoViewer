use super::*;
use crate::ui::keyboard::binding::resolve_binding;

/// Modifier combinations a binding table can plausibly match on. `alt` covers
/// the `Meta` alias because `KeyCombo::new` folds the two together.
const MODIFIER_SETS: [(bool, bool, bool); 5] = [
    (false, false, false),
    (true, false, false),
    (false, true, false),
    (true, true, false),
    (false, false, true),
];

fn probe_combos() -> Vec<KeyCombo> {
    let keys = [
        gdk::Key::Escape,
        gdk::Key::Left,
        gdk::Key::Right,
        gdk::Key::Up,
        gdk::Key::Down,
        gdk::Key::KP_Left,
        gdk::Key::KP_Right,
        gdk::Key::KP_Up,
        gdk::Key::KP_Down,
        gdk::Key::Return,
        gdk::Key::KP_Enter,
        gdk::Key::space,
        gdk::Key::KP_Space,
        gdk::Key::Delete,
        gdk::Key::a,
        gdk::Key::A,
        gdk::Key::f,
        gdk::Key::F,
        gdk::Key::i,
        gdk::Key::e,
        gdk::Key::h,
        gdk::Key::r,
        gdk::Key::R,
        gdk::Key::_0,
        gdk::Key::KP_0,
        gdk::Key::plus,
        gdk::Key::equal,
        gdk::Key::KP_Add,
        gdk::Key::minus,
        gdk::Key::KP_Subtract,
        gdk::Key::comma,
        gdk::Key::slash,
        gdk::Key::question,
        gdk::Key::F1,
    ];

    keys.iter()
        .flat_map(|key| {
            MODIFIER_SETS
                .iter()
                .map(move |(ctrl, shift, alt)| KeyCombo {
                    key: *key,
                    ctrl: *ctrl,
                    shift: *shift,
                    alt: *alt,
                })
        })
        .collect()
}

fn declared(scope: KeyboardScope) -> impl Fn(KeyboardAction) -> bool + Copy {
    move |action| {
        GROUPS
            .iter()
            .find(|group| group.scope == scope)
            .is_some_and(|group| group.shortcuts.iter().any(|s| s.action == action))
    }
}

/// Forward direction: a row that advertises `F1 → shortcuts window` must be
/// what the router actually does, or the reference is lying to the user.
#[gtk::test]
fn every_declared_accelerator_opens_the_action_it_claims() {
    for group in GROUPS {
        for shortcut in group.shortcuts {
            let combo = combo_for(shortcut.accelerator).unwrap_or_else(|| {
                panic!(
                    "{:?} is not a parseable accelerator (group {:?})",
                    shortcut.accelerator, group.scope
                )
            });
            assert_eq!(
                resolve_binding(group.scope, combo),
                Some(shortcut.action),
                "{} resolves elsewhere in {:?} — the window claims {:?}",
                shortcut.accelerator,
                group.scope,
                shortcut.action,
            );
        }
    }
}

/// Reverse direction: the highest-value half of the guard. Bind a new key in
/// `binding.rs`, forget the window, and this goes red.
#[gtk::test]
fn every_routable_action_has_a_row_in_the_window() {
    for group in GROUPS {
        for combo in probe_combos() {
            let Some(action) = resolve_binding(group.scope, combo) else {
                continue;
            };
            if declared(group.scope)(action) {
                continue;
            }
            // `resolve_binding` falls through to the Global table for Viewer and
            // Browsing; a fall-through action is documented when the Global
            // group lists it, so it does not need duplicating per scope.
            let documented_by_fallback = group.scope != Global
                && resolve_binding(Global, combo) == Some(action)
                && declared(Global)(action);
            assert!(
                documented_by_fallback,
                "{:?} in {:?} is reachable but has no row in the shortcuts window",
                action, group.scope,
            );
        }
    }
}

/// The discoverability entry itself: both conventions must work, because
/// neither is documented anywhere else in the app.
#[gtk::test]
fn the_reference_window_is_reachable_by_both_conventions() {
    for accelerator in ["F1", "<Ctrl>slash", "<Ctrl>question"] {
        let combo = combo_for(accelerator).expect("parseable");
        assert_eq!(
            resolve_binding(KeyboardScope::Global, combo),
            Some(KeyboardAction::ShowShortcuts),
            "{accelerator} should open the shortcut reference",
        );
    }
}

#[gtk::test]
fn every_row_label_is_translated() {
    for group in GROUPS {
        assert_ne!(tr(group.title_key), group.title_key);
        for shortcut in group.shortcuts {
            assert_ne!(
                tr(shortcut.label_key),
                shortcut.label_key,
                "{:?} has no i18n entry",
                shortcut.label_key,
            );
        }
    }
}

#[gtk::test]
fn tooltip_hints_reuse_the_reference_table() {
    // A button tooltip and the window must never claim different keys.
    assert_eq!(
        display_accelerator(accelerator_for(ViewerZoomIn).expect("row")),
        "+"
    );
    assert!(
        tooltip_with_key("viewer.tooltip.zoom_in", ViewerZoomIn).ends_with(" (+)"),
        "the zoom tooltip should carry its key: {:?}",
        tooltip_with_key("viewer.tooltip.zoom_in", ViewerZoomIn),
    );
    assert_eq!(
        tooltip_with_key("viewer.tooltip.play_motion_photo", Restore),
        tr("viewer.tooltip.play_motion_photo"),
        "an action with no row keeps a plain label instead of a broken hint"
    );
}

#[gtk::test]
fn accelerators_render_the_way_a_user_reads_them() {
    for (accelerator, expected) in [
        ("F1", "F1"),
        ("<Ctrl>F", "Ctrl+F"),
        ("<Ctrl>slash", "Ctrl+/"),
        ("<Ctrl>comma", "Ctrl+,"),
        ("<Alt>Left", "Alt+←"),
        ("Escape", "Esc"),
        ("Return", "Enter"),
        ("space", "Space"),
        ("Delete", "Del"),
        ("Up", "↑"),
        ("KP_Subtract", "Num+-"),
        ("<Shift>R", "Shift+R"),
        ("r", "R"),
        ("0", "0"),
    ] {
        assert_eq!(display_accelerator(accelerator), expected, "{accelerator}");
    }

    // The window renders through gtk_accelerator_get_label, which capitalises
    // letters and keeps Shift visible; the rotate pair has to stay separable in
    // tooltips too, or one glyph advertises two different actions.
    assert_eq!(
        tooltip_with_key("keyboard.rotate_right", ViewerRotateRight),
        format!("{} (R)", tr("keyboard.rotate_right"))
    );
    assert_eq!(
        tooltip_with_key("keyboard.rotate_left", ViewerRotateLeft),
        format!("{} (Shift+R)", tr("keyboard.rotate_left"))
    );
}

/// The skeleton is generated, so prove it really carries one object per row
/// before any text is poured into it.
#[gtk::test]
fn the_generated_skeleton_has_one_object_per_declared_row() {
    let builder = gtk::Builder::new();
    builder
        .add_from_string(&skeleton())
        .expect("generated shortcut UI must parse");

    let objects = builder.objects();
    let count = |wanted: &str| {
        objects
            .iter()
            .filter(|object| object.type_().name() == wanted)
            .count()
    };
    let rows: usize = GROUPS.iter().map(|group| group.shortcuts.len()).sum();
    assert_eq!(count("GtkShortcutsWindow"), 1);
    assert_eq!(count("GtkShortcutsSection"), 1);
    assert_eq!(count("GtkShortcutsGroup"), GROUPS.len());
    assert_eq!(count("GtkShortcutsShortcut"), rows);

    // Every declared id must be addressable, because `open` fills text in by id.
    for (group_index, group) in GROUPS.iter().enumerate() {
        assert!(
            builder
                .object::<gtk::ShortcutsGroup>(&group_id(group_index))
                .is_some(),
            "group {group_index} has no object",
        );
        for index in 0..group.shortcuts.len() {
            assert!(
                builder
                    .object::<gtk::ShortcutsShortcut>(&shortcut_id(group_index, index))
                    .is_some(),
                "row {group_index}-{index} has no object",
            );
        }
    }
}

/// Labels are properties set after the parse, never XML text, so a translation
/// containing markup cannot break the build.
#[gtk::test]
fn translated_text_is_attached_as_properties() {
    let builder = gtk::Builder::new();
    builder.add_from_string(&skeleton()).expect("parses");
    let row = builder
        .object::<gtk::ShortcutsShortcut>(&shortcut_id(0, 0))
        .expect("first row");
    row.set_property("accelerator", "<Ctrl>slash");
    row.set_property("title", "放大 & 缩小 <b>");
    assert_eq!(
        row.property::<gtk::glib::GString>("accelerator").as_str(),
        "<Ctrl>slash"
    );
    assert_eq!(
        row.property::<gtk::glib::GString>("title").as_str(),
        "放大 & 缩小 <b>"
    );
}

#[gtk::test]
fn opening_builds_a_modal_window_parented_to_the_caller() {
    let parent = gtk::Window::new();
    let window = open(&parent).expect("the reference opens");

    assert!(window.is_modal(), "the reference must be modal");
    assert_eq!(
        window.transient_for().as_ref().map(|w| w.as_ptr()),
        Some(parent.as_ptr()),
        "the reference must stay parented to the window it came from"
    );
    assert_eq!(window.view_name().as_deref(), Some(VIEW));
    assert_eq!(
        window.default_height(),
        520,
        "the reference holds {} rows across {} groups",
        GROUPS.iter().map(|g| g.shortcuts.len()).sum::<usize>(),
        GROUPS.len(),
    );
    assert_eq!(
        window.title().as_deref(),
        Some(tr("keyboard.window.title").as_str())
    );
    // GtkBuilder owns the objects it constructs, and it is gone by now: GTK has
    // to keep the window alive and mapped on its own.
    assert!(
        window.surface().is_some(),
        "the reference must outlive its builder"
    );
    assert!(
        gtk::Window::list_toplevels()
            .iter()
            .any(|w| *w == window.clone().upcast::<gtk::Widget>()),
        "the reference must be a real toplevel"
    );

    // Constructed is not displayed: the section only shows when its
    // `section-name` matches the window `view-name`, and every row has to
    // participate in the layout.
    let content = window.child().expect("the reference holds content");
    assert!(
        content.is_child_visible(),
        "the section is hidden — section-name and view-name disagree"
    );
    let (_, natural_height, _, _) = content.measure(gtk::Orientation::Vertical, -1);
    let rows: usize = GROUPS.iter().map(|g| g.shortcuts.len()).sum();
    assert!(
        (natural_height as usize) > rows * 10,
        "{} rows should lay out to more than {}px, got {natural_height}",
        rows,
        rows * 10,
    );
    window.close();
}
