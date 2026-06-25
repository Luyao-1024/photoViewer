# Viewer Module

## Scope

The viewer module covers the full photo view, top overlay toolbar, left/right navigation, bottom thumbnail strip, details panel, and editor entry points.

## Key Files

| File | Role |
|---|---|
| `src/ui/viewer_page.rs` | Viewer state, navigation, overlay panel behavior |
| `data/ui/viewer-page.blp` | Viewer template |
| `tests/e2e_viewer.rs` | Viewer flow coverage |
| `tests/ui_viewer_toolbar.rs` | Viewer toolbar/template assertions |

## Layout Contract

The viewer is pushed inside the existing `adw::NavigationView`; it must not resize the main app sidebar. Keep viewer chrome inside the page content area and avoid constraints that alter root window/sidebar sizing.

Overlay controls should have stable dimensions. Hidden panels should not leave child content measured in a collapsed allocation path, because that can produce warnings such as negative width or height in `gtk_widget_size_allocate`.

## Thumbnail Strip

The thumbnail strip should initialize centered on the active image. If centering only happens after user interaction, the adjustment is being applied before the widget has a final allocation; schedule the centering after layout or after the thumbnail model is populated.

## Navigation Buttons

Left/right image navigation belongs to viewer chrome. When positioned as top-right overlay controls, keep them visually consistent with the active material mode and avoid blocking the original image more than necessary.

## Details And Editor Panels

Details/editor side panels should be treated as overlay chrome, not as layout that changes the main image viewport unexpectedly. Collapsed state should hide or unparent expensive/size-forcing child regions where needed.
