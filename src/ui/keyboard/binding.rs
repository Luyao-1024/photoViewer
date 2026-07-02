use gtk4::gdk;

use super::action::KeyboardAction;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeyboardScope {
    TextInput,
    Modal,
    Editor,
    Viewer,
    Browsing,
    Global,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyCombo {
    pub key: gdk::Key,
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
}

impl KeyCombo {
    pub fn new(key: gdk::Key, state: gdk::ModifierType) -> Self {
        Self {
            key,
            ctrl: state.contains(gdk::ModifierType::CONTROL_MASK),
            shift: state.contains(gdk::ModifierType::SHIFT_MASK),
            alt: state.contains(gdk::ModifierType::ALT_MASK)
                || state.contains(gdk::ModifierType::META_MASK),
        }
    }

    pub fn plain(key: gdk::Key) -> Self {
        Self {
            key,
            ctrl: false,
            shift: false,
            alt: false,
        }
    }
}

pub fn resolve_binding(scope: KeyboardScope, combo: KeyCombo) -> Option<KeyboardAction> {
    scoped_binding(scope, combo).or_else(|| {
        if matches!(
            scope,
            KeyboardScope::Global
                | KeyboardScope::TextInput
                | KeyboardScope::Modal
                | KeyboardScope::Editor
        ) {
            None
        } else {
            scoped_binding(KeyboardScope::Global, combo)
        }
    })
}

fn scoped_binding(scope: KeyboardScope, combo: KeyCombo) -> Option<KeyboardAction> {
    use KeyboardAction::*;
    use KeyboardScope::*;

    match scope {
        TextInput => None,
        Modal | Editor => match combo {
            c if c == KeyCombo::plain(gdk::Key::Escape) => Some(CancelOrClose),
            _ => None,
        },
        Viewer => viewer_binding(combo),
        Browsing => browsing_binding(combo),
        Global => global_binding(combo),
    }
}

fn global_binding(combo: KeyCombo) -> Option<KeyboardAction> {
    use KeyboardAction::*;

    match combo {
        c if c == KeyCombo::plain(gdk::Key::Escape) => Some(CancelOrClose),
        KeyCombo {
            key: gdk::Key::Left | gdk::Key::KP_Left,
            alt: true,
            ctrl: false,
            shift: false,
        } => Some(NavigateBack),
        KeyCombo {
            key: gdk::Key::f,
            ctrl: true,
            alt: false,
            shift: false,
        }
        | KeyCombo {
            key: gdk::Key::F,
            ctrl: true,
            alt: false,
            shift: true,
        } => Some(Search),
        KeyCombo {
            key: gdk::Key::comma,
            ctrl: true,
            alt: false,
            shift: false,
        } => Some(OpenSettings),
        _ => None,
    }
}

fn browsing_binding(combo: KeyCombo) -> Option<KeyboardAction> {
    use KeyboardAction::*;

    match combo {
        c if c == KeyCombo::plain(gdk::Key::Up) || c == KeyCombo::plain(gdk::Key::KP_Up) => {
            Some(BrowseUp)
        }
        c if c == KeyCombo::plain(gdk::Key::Down) || c == KeyCombo::plain(gdk::Key::KP_Down) => {
            Some(BrowseDown)
        }
        c if c == KeyCombo::plain(gdk::Key::Left) || c == KeyCombo::plain(gdk::Key::KP_Left) => {
            Some(BrowseLeft)
        }
        c if c == KeyCombo::plain(gdk::Key::Right) || c == KeyCombo::plain(gdk::Key::KP_Right) => {
            Some(BrowseRight)
        }
        c if c == KeyCombo::plain(gdk::Key::Return) || c == KeyCombo::plain(gdk::Key::KP_Enter) => {
            Some(ActivateFocused)
        }
        c if c == KeyCombo::plain(gdk::Key::space) || c == KeyCombo::plain(gdk::Key::KP_Space) => {
            Some(ToggleSelection)
        }
        KeyCombo {
            key: gdk::Key::a,
            ctrl: true,
            alt: false,
            shift: false,
        }
        | KeyCombo {
            key: gdk::Key::A,
            ctrl: true,
            alt: false,
            shift: true,
        } => Some(SelectAll),
        c if c == KeyCombo::plain(gdk::Key::Delete) => Some(Delete),
        _ => None,
    }
}

fn viewer_binding(combo: KeyCombo) -> Option<KeyboardAction> {
    use KeyboardAction::*;

    match combo {
        c if c == KeyCombo::plain(gdk::Key::Left) || c == KeyCombo::plain(gdk::Key::KP_Left) => {
            Some(ViewerPrevious)
        }
        c if c == KeyCombo::plain(gdk::Key::Right) || c == KeyCombo::plain(gdk::Key::KP_Right) => {
            Some(ViewerNext)
        }
        c if c == KeyCombo::plain(gdk::Key::Escape) => Some(CancelOrClose),
        c if c == KeyCombo::plain(gdk::Key::space) || c == KeyCombo::plain(gdk::Key::KP_Space) => {
            Some(ViewerTogglePlayback)
        }
        KeyCombo {
            key: gdk::Key::plus | gdk::Key::KP_Add | gdk::Key::equal,
            ctrl: false,
            alt: false,
            ..
        } => Some(ViewerZoomIn),
        c if c == KeyCombo::plain(gdk::Key::minus)
            || c == KeyCombo::plain(gdk::Key::KP_Subtract) =>
        {
            Some(ViewerZoomOut)
        }
        c if c == KeyCombo::plain(gdk::Key::_0) || c == KeyCombo::plain(gdk::Key::KP_0) => {
            Some(ViewerZoomReset)
        }
        c if c == KeyCombo::plain(gdk::Key::r) => Some(ViewerRotateRight),
        KeyCombo {
            key: gdk::Key::R,
            ctrl: false,
            alt: false,
            shift: true,
        } => Some(ViewerRotateLeft),
        c if c == KeyCombo::plain(gdk::Key::f) => Some(ViewerFullscreenPreview),
        c if c == KeyCombo::plain(gdk::Key::i) => Some(ViewerToggleDetails),
        c if c == KeyCombo::plain(gdk::Key::e) => Some(ViewerToggleEdit),
        c if c == KeyCombo::plain(gdk::Key::h) => Some(ViewerToggleFavorite),
        c if c == KeyCombo::plain(gdk::Key::Delete) => Some(Delete),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
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
}
