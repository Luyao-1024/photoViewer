use super::*;
use KeyboardAction::*;
use KeyboardScope::*;

#[test]
fn viewer_arrows_resolve_to_media_navigation() {
    assert_eq!(
        resolve_binding(Viewer, KeyCombo::plain(gdk::Key::Right)),
        Some(ViewerNext)
    );
    assert_eq!(
        resolve_binding(Viewer, KeyCombo::plain(gdk::Key::Left)),
        Some(ViewerPrevious)
    );
}

#[test]
fn viewer_chrome_shortcuts_resolve_to_actions() {
    assert_eq!(
        resolve_binding(Viewer, KeyCombo::plain(gdk::Key::space)),
        Some(ViewerTogglePlayback)
    );
    assert_eq!(
        resolve_binding(Viewer, KeyCombo::plain(gdk::Key::plus)),
        Some(ViewerZoomIn)
    );
    assert_eq!(
        resolve_binding(
            Viewer,
            KeyCombo::new(gdk::Key::plus, gdk::ModifierType::SHIFT_MASK)
        ),
        Some(ViewerZoomIn)
    );
    assert_eq!(
        resolve_binding(
            Viewer,
            KeyCombo::new(gdk::Key::equal, gdk::ModifierType::SHIFT_MASK)
        ),
        Some(ViewerZoomIn)
    );
    assert_eq!(
        resolve_binding(
            Viewer,
            KeyCombo::new(gdk::Key::R, gdk::ModifierType::SHIFT_MASK)
        ),
        Some(ViewerRotateLeft)
    );
    assert_eq!(
        resolve_binding(Viewer, KeyCombo::plain(gdk::Key::i)),
        Some(ViewerToggleDetails)
    );
    assert_eq!(
        resolve_binding(Viewer, KeyCombo::plain(gdk::Key::h)),
        Some(ViewerToggleFavorite)
    );
}

#[test]
fn browsing_arrows_resolve_to_grid_navigation() {
    assert_eq!(
        resolve_binding(Browsing, KeyCombo::plain(gdk::Key::Right)),
        Some(BrowseRight)
    );
    assert_eq!(
        resolve_binding(Browsing, KeyCombo::plain(gdk::Key::Up)),
        Some(BrowseUp)
    );
}

#[test]
fn text_input_suppresses_printable_app_shortcuts() {
    assert_eq!(
        resolve_binding(TextInput, KeyCombo::plain(gdk::Key::f)),
        None
    );
    assert_eq!(
        resolve_binding(
            TextInput,
            KeyCombo::new(gdk::Key::f, gdk::ModifierType::CONTROL_MASK)
        ),
        None
    );
}

#[test]
fn non_modal_non_text_scopes_fall_back_to_global_actions() {
    assert_eq!(
        resolve_binding(
            Viewer,
            KeyCombo::new(gdk::Key::comma, gdk::ModifierType::CONTROL_MASK)
        ),
        Some(OpenSettings)
    );
    assert_eq!(
        resolve_binding(
            Browsing,
            KeyCombo::new(gdk::Key::f, gdk::ModifierType::CONTROL_MASK)
        ),
        Some(Search)
    );
}

#[test]
fn modal_and_editor_scopes_do_not_fall_back_to_global_actions() {
    assert_eq!(
        resolve_binding(
            Modal,
            KeyCombo::new(gdk::Key::f, gdk::ModifierType::CONTROL_MASK)
        ),
        None
    );
    assert_eq!(
        resolve_binding(
            Editor,
            KeyCombo::new(gdk::Key::comma, gdk::ModifierType::CONTROL_MASK)
        ),
        None
    );
    assert_eq!(
        resolve_binding(Modal, KeyCombo::plain(gdk::Key::Escape)),
        Some(CancelOrClose)
    );
}
