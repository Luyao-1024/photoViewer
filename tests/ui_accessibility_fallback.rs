//! The grid CSS provider defines `prefers-reduced-transparency: reduce` and
//! `prefers-contrast: more` @media blocks that override the glass materials
//! with stable opaque surfaces. This is the platform-level accessibility
//! fallback (per the spec's section 7) and is opt-out: when the media query
//! does NOT match, the rules are ignored and the glass look is unchanged.
//!
//! GTK is single-threaded; all checks live in one `#[test]` function.

use gtk4 as gtk;
use gtk4::prelude::*;
use photo_viewer::ui::grid_css;

#[test]
fn accessibility_fallbacks_present() {
    gtk::init().expect("GTK init failed");
    grid_css::install();

    let css = grid_css::css_for_tests();

    // Reduced-transparency block must exist and must override at least
    // .glass-base and .glass-raised with opaque backgrounds.
    assert!(
        css.contains("@media (prefers-reduced-transparency: reduce)"),
        "GRID_CSS must contain a reduced-transparency @media block"
    );
    let block_start = css
        .find("@media (prefers-reduced-transparency: reduce)")
        .expect("reduced-transparency block not present");
    // Slice only the reduced-transparency block (up to the next @media
    // or the file end). Skip past the opening `@media` so we don't match
    // it again.
    let search_start = block_start + 1;
    let block_end = css[search_start..]
        .find("@media ")
        .map(|off| search_start + off)
        .unwrap_or(css.len());
    let block = &css[block_start..block_end];
    assert!(
        block.contains(".glass-base"),
        "reduced-transparency block must override .glass-base"
    );
    assert!(
        block.contains(".glass-raised"),
        "reduced-transparency block must override .glass-raised"
    );
    // backdrop-filter must be explicitly disabled (not just dimmed) inside
    // the reduced-transparency block.
    assert!(
        block.contains("backdrop-filter: none"),
        "reduced-transparency block must contain `backdrop-filter: none`, got block:\n{block}"
    );

    // High-contrast block must exist and must force thicker borders.
    assert!(
        css.contains("@media (prefers-contrast: more)"),
        "GRID_CSS must contain a high-contrast @media block"
    );
    let block_start = css
        .find("@media (prefers-contrast: more)")
        .expect("high-contrast block not present");
    // Last block: take everything from block_start to the file end.
    let block = &css[block_start..];
    assert!(
        block.contains("border: 2px solid"),
        "high-contrast block must force thicker opaque borders, got {block}"
    );
    assert!(
        block.contains(".glass-base"),
        "high-contrast block must override .glass-base"
    );
    assert!(
        block.contains(".glass-raised"),
        "high-contrast block must override .glass-raised"
    );
}
