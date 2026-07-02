#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeyboardAction {
    CancelOrClose,
    NavigateBack,
    BrowseUp,
    BrowseDown,
    BrowseLeft,
    BrowseRight,
    ActivateFocused,
    ToggleSelection,
    SelectAll,
    Search,
    OpenSettings,
    Delete,
    Restore,
    ViewerPrevious,
    ViewerNext,
    ViewerZoomIn,
    ViewerZoomOut,
    ViewerZoomReset,
    ViewerRotateLeft,
    ViewerRotateRight,
    ViewerFullscreenPreview,
    ViewerToggleDetails,
    ViewerToggleEdit,
    ViewerToggleFavorite,
    ViewerTogglePlayback,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyboardResult {
    Handled,
    Ignored,
}

impl KeyboardResult {
    pub fn is_handled(self) -> bool {
        matches!(self, Self::Handled)
    }
}
