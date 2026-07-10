use super::super::test_support::*;
use super::super::*;
use super::*;

use std::sync::Arc;

#[gtk::test]
fn section_flowbox_selection_mode_tracks_multi_select() {
    // The checkmark is revealed by `flowboxchild:selected`, which can only
    // happen while a section FlowBox is in `Multiple`. Out of multi-select
    // every section must be `None` so no stray tick can appear.
    let _ = gtk::init();
    let dir = tempfile::tempdir().unwrap();
    let pool = crate::core::db::init_pool(&dir.path().join("test.db")).unwrap();
    let loader = Arc::new(ThumbnailLoader::new(pool, dir.path().join("thumbs")));
    let media_list = gio::ListStore::new::<glib::BoxedAnyObject>();
    media_list.append(&glib::BoxedAnyObject::new(sample_item(1, "one.png")));
    media_list.append(&glib::BoxedAnyObject::new(sample_item(2, "two.png")));

    let grid = MediaGrid::new(media_list, GroupBy::Day, loader, noop_callbacks(), false);

    let modes = section_flow_selection_modes(&grid);
    assert!(
        !modes.is_empty(),
        "rebuild should produce section FlowBoxes"
    );
    assert!(
        modes.iter().all(|m| *m == gtk::SelectionMode::None),
        "default (non-multi) must be None, got {modes:?}"
    );

    grid.set_multi_select_mode(true);
    let modes = section_flow_selection_modes(&grid);
    assert!(
        modes.iter().all(|m| *m == gtk::SelectionMode::Multiple),
        "multi-select must flip every section to Multiple, got {modes:?}"
    );

    grid.set_multi_select_mode(false);
    let modes = section_flow_selection_modes(&grid);
    assert!(
        modes.iter().all(|m| *m == gtk::SelectionMode::None),
        "exiting multi-select must restore None, got {modes:?}"
    );
}
