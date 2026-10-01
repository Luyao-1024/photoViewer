//! The in-app keyboard shortcut reference.
//!
//! The router in `binding.rs` already binds ~25 keys, but nothing in the app
//! ever showed them, so the keyboard layer was invisible to anyone who had not
//! read `docs/modules/keyboard.md`. This module renders `GtkShortcutsWindow`
//! from [`GROUPS`], and `tests` asserts both directions: every accelerator
//! listed here really resolves to the action it claims, and every action the
//! router can reach in a given scope is listed there. Adding a binding without
//! a row (or the other way round) fails the test.
//!
//! The window is rebuilt on every open rather than cached: the app can switch
//! locale at runtime, and a stale translated window is worse than a few dozen
//! rows built on demand.
use gtk4 as gtk;
use gtk4::gdk;
use gtk4::prelude::*;

use super::action::KeyboardAction;
use super::binding::{KeyCombo, KeyboardScope};
use crate::core::i18n::tr;

/// One row of the reference. `accelerator` is a GTK accelerator string
/// (`<Ctrl>F`, `plus`, `F1`) — the same syntax `binding.rs` matches on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShortcutSpec {
    pub action: KeyboardAction,
    pub accelerator: &'static str,
    pub label_key: &'static str,
}

/// One `GtkShortcutsGroup`. `scope` is the router scope the rows must resolve
/// in, which is what makes the drift test two-directional.
#[derive(Debug, Clone, Copy)]
pub struct ShortcutGroup {
    pub scope: KeyboardScope,
    pub title_key: &'static str,
    pub shortcuts: &'static [ShortcutSpec],
}

use KeyboardAction::*;
use KeyboardScope::*;

pub const GROUPS: &[ShortcutGroup] = &[
    ShortcutGroup {
        scope: Global,
        title_key: "keyboard.group.global",
        shortcuts: &[
            spec(ShowShortcuts, "F1", "keyboard.show_shortcuts"),
            spec(ShowShortcuts, "<Ctrl>slash", "keyboard.show_shortcuts"),
            spec(CancelOrClose, "Escape", "keyboard.cancel_or_close"),
            spec(NavigateBack, "<Alt>Left", "keyboard.navigate_back"),
            spec(Search, "<Ctrl>F", "keyboard.search"),
            spec(OpenSettings, "<Ctrl>comma", "keyboard.settings"),
        ],
    },
    ShortcutGroup {
        scope: Browsing,
        title_key: "keyboard.group.browsing",
        shortcuts: &[
            spec(BrowseUp, "Up", "keyboard.move_focus_up"),
            spec(BrowseDown, "Down", "keyboard.move_focus_down"),
            spec(BrowseLeft, "Left", "keyboard.move_focus_left"),
            spec(BrowseRight, "Right", "keyboard.move_focus_right"),
            spec(ActivateFocused, "Return", "keyboard.open_focused"),
            spec(ToggleSelection, "space", "keyboard.toggle_selection"),
            spec(SelectAll, "<Ctrl>A", "keyboard.select_all"),
            spec(Delete, "Delete", "keyboard.move_to_trash"),
        ],
    },
    ShortcutGroup {
        scope: Viewer,
        title_key: "keyboard.group.viewer",
        shortcuts: &[
            spec(ViewerPrevious, "Left", "keyboard.previous_media"),
            spec(ViewerNext, "Right", "keyboard.next_media"),
            spec(CancelOrClose, "Escape", "keyboard.close_viewer"),
            spec(ViewerTogglePlayback, "space", "keyboard.toggle_playback"),
            spec(ViewerZoomIn, "plus", "keyboard.zoom_in"),
            spec(ViewerZoomOut, "minus", "keyboard.zoom_out"),
            spec(ViewerZoomReset, "0", "keyboard.zoom_reset"),
            spec(ViewerRotateRight, "r", "keyboard.rotate_right"),
            spec(ViewerRotateLeft, "<Shift>R", "keyboard.rotate_left"),
            spec(ViewerFullscreenPreview, "f", "keyboard.fullscreen_preview"),
            spec(ViewerToggleDetails, "i", "keyboard.toggle_details"),
            spec(ViewerToggleEdit, "e", "keyboard.toggle_edit"),
            spec(ViewerToggleFavorite, "h", "keyboard.toggle_favorite"),
            spec(Delete, "Delete", "keyboard.move_to_trash"),
        ],
    },
];

const fn spec(
    action: KeyboardAction,
    accelerator: &'static str,
    label_key: &'static str,
) -> ShortcutSpec {
    ShortcutSpec {
        action,
        accelerator,
        label_key,
    }
}

/// Parse a declared accelerator into the combo the router sees.
pub fn combo_for(accelerator: &str) -> Option<KeyCombo> {
    let (key, mods) = gtk::accelerator_parse(accelerator)?;
    if key == gdk::Key::VoidSymbol {
        return None;
    }
    Some(KeyCombo::new(key, mods))
}

/// The accelerator the reference advertises for `action`, if it has one.
/// Actions with no binding (`Restore`, which only the trash toolbar offers)
/// return `None`, so callers can degrade to a label without a hint.
pub fn accelerator_for(action: KeyboardAction) -> Option<&'static str> {
    GROUPS
        .iter()
        .flat_map(|group| group.shortcuts)
        .find(|shortcut| shortcut.action == action)
        .map(|shortcut| shortcut.accelerator)
}

/// Tooltips spell a key the way a user reads it, not the way
/// `gtk_accelerator_parse` spells it: `↑` beats `Up`, `Ctrl+F` beats `<Ctrl>F`.
///
/// Modifier and letter rules match `gtk_accelerator_get_label` (which the
/// reference window itself uses), so `<Shift>R` reads `Shift+R` here and there.
/// Non-printable keys deliberately keep our own glyphs — GTK renders `Up` as
/// `上` in a Chinese locale, which is a poor hint to float over a button.
pub fn display_accelerator(accelerator: &str) -> String {
    let (key, mods) = gtk::accelerator_parse(accelerator)
        .unwrap_or((gdk::Key::VoidSymbol, gdk::ModifierType::empty()));

    let mut parts: Vec<&'static str> = Vec::new();
    if mods.contains(gdk::ModifierType::CONTROL_MASK) {
        parts.push("Ctrl");
    }
    if mods.intersects(gdk::ModifierType::ALT_MASK | gdk::ModifierType::META_MASK) {
        parts.push("Alt");
    }
    if mods.contains(gdk::ModifierType::SHIFT_MASK) {
        parts.push("Shift");
    }

    let mut name = key_name(key).to_string();
    if name.is_empty() {
        name = key
            .to_unicode()
            .map(|ch| ch.to_string())
            .or_else(|| key.name().map(|n| n.to_string()))
            .unwrap_or_default();
    }

    // Folding Shift away used to make rotate-left read `R`, the glyph
    // rotate-right already owns; both actions then shared one hint.
    if name.chars().nth(1).is_none() {
        name = name.to_uppercase();
    }

    if parts.is_empty() {
        name
    } else {
        format!("{}+{}", parts.join("+"), name)
    }
}

/// Keys with no printable character. Everything else resolves through
/// `gdk::Key::to_unicode`, which already yields `+`, `-`, `,`, `/`, `0`.
fn key_name(key: gdk::Key) -> &'static str {
    match key {
        gdk::Key::Up => "↑",
        gdk::Key::Down => "↓",
        gdk::Key::Left => "←",
        gdk::Key::Right => "→",
        gdk::Key::KP_Up => "Num+↑",
        gdk::Key::KP_Down => "Num+↓",
        gdk::Key::KP_Left => "Num+←",
        gdk::Key::KP_Right => "Num+→",
        gdk::Key::KP_Add => "Num++",
        gdk::Key::KP_Subtract => "Num+-",
        gdk::Key::Escape => "Esc",
        gdk::Key::Delete => "Del",
        gdk::Key::Return | gdk::Key::KP_Enter => "Enter",
        gdk::Key::space | gdk::Key::KP_Space => "Space",
        _ => "",
    }
}

/// Translated tooltip carrying its shortcut, e.g. `放大 (+)` — the same
/// accelerator the reference window lists, so the two cannot disagree.
pub fn tooltip_with_key(label_key: &str, action: KeyboardAction) -> String {
    let label = tr(label_key);
    let Some(accelerator) = accelerator_for(action) else {
        return label;
    };
    format!("{label} ({})", display_accelerator(accelerator))
}

/// Show the reference, parented to `parent` so window management, theming and
/// the Escape-to-close path all stay with the main window. The window is
/// returned so callers (and tests) can hold the same instance the builder made.
pub fn open(parent: &impl IsA<gtk::Window>) -> Option<gtk::ShortcutsWindow> {
    let builder = gtk::Builder::new();
    if let Err(error) = builder.add_from_string(&skeleton()) {
        tracing::error!(%error, "shortcut reference UI could not be built");
        return None;
    }

    let Some(window) = builder.object::<gtk::ShortcutsWindow>(WINDOW_ID) else {
        tracing::error!("shortcut reference skeleton has no window object");
        return None;
    };
    window.set_title(Some(&tr("keyboard.window.title")));

    for (group_index, group) in GROUPS.iter().enumerate() {
        let Some(row) = builder.object::<gtk::ShortcutsGroup>(&group_id(group_index)) else {
            tracing::error!("shortcut reference lost group {group_index}");
            continue;
        };
        row.set_title(Some(&tr(group.title_key)));
        for (index, shortcut) in group.shortcuts.iter().enumerate() {
            let Some(item) =
                builder.object::<gtk::ShortcutsShortcut>(&shortcut_id(group_index, index))
            else {
                tracing::error!("shortcut reference lost row {group_index}-{index}");
                continue;
            };
            item.set_property("accelerator", shortcut.accelerator);
            item.set_property("title", tr(shortcut.label_key));
        }
    }

    window.set_transient_for(Some(parent));
    window.present();
    Some(window)
}

/// The widget skeleton: classes and ids only, with the real content filled in
/// from [`GROUPS`] afterwards.
///
/// Translated strings never enter the XML, so a catalog containing `&` or `<`
/// cannot break the parse. That also keeps this working on the crate's `v4_8`
/// gtk4 feature: `ShortcutsWindow::add_section` and its friends need `v4_14`,
/// and enabling that turns every `Widget::allocation` and
/// `CssProvider::load_from_data` use in this crate into a hard error under
/// CI's `-D warnings`.
fn skeleton() -> String {
    let mut xml = format!(
        "<interface>\
         <object class=\"GtkShortcutsWindow\" id=\"{WINDOW_ID}\">\
         <property name=\"view-name\">{VIEW}</property>\
         <property name=\"modal\">True</property>\
         <property name=\"destroy-with-parent\">True</property>\
         <property name=\"default-width\">560</property>\
         <property name=\"default-height\">520</property>\
         <child>\
         <object class=\"GtkShortcutsSection\" id=\"{SECTION_ID}\">\
         <property name=\"section-name\">{VIEW}</property>\
         <property name=\"max-height\">520</property>"
    );

    for (group_index, group) in GROUPS.iter().enumerate() {
        xml.push_str(&format!(
            "<child><object class=\"GtkShortcutsGroup\" id=\"{}\">",
            group_id(group_index),
        ));
        for (index, _) in group.shortcuts.iter().enumerate() {
            xml.push_str(&format!(
                "<child><object class=\"GtkShortcutsShortcut\" id=\"{}\"/></child>",
                shortcut_id(group_index, index),
            ));
        }
        xml.push_str("</object></child>");
    }

    xml.push_str("</object></child></object></interface>");
    xml
}

fn group_id(index: usize) -> String {
    format!("pv-shortcut-group-{index}")
}

fn shortcut_id(group_index: usize, index: usize) -> String {
    format!("pv-shortcut-{group_index}-{index}")
}

/// The single view name shared by the window and its section: a
/// `GtkShortcutsSection` is only shown when its `section-name` matches the
/// window's `view-name`.
const VIEW: &str = "shortcuts";
const WINDOW_ID: &str = "pv-shortcut-reference-window";
const SECTION_ID: &str = "pv-shortcut-reference-section";

#[cfg(test)]
mod tests;
