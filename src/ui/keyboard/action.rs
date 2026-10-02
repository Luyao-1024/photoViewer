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
    ShowShortcuts,
    Delete,
    Restore,
    ViewerPrevious,
    ViewerNext,
    ViewerZoomIn,
    ViewerZoomOut,
    ViewerZoomReset,
    ViewerRotateLeft,
    ViewerRotateRight,
    /// In-place immersive browsing: the viewer's own chrome folds away so the
    /// picture owns the page. Distinct from [`Self::ViewerFullscreenPreview`],
    /// which is the separate system-fullscreen window.
    ViewerImmersive,
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
