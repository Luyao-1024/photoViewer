//! A local import must not flash the whole grid the way a full reload does.
mod common;
use common::{
    interaction::descendants,
    shell::{write_photo, Shell},
};
use gtk::prelude::*;
use gtk4 as gtk;
use photo_viewer::core::{
    backend::local::LocalBackend,
    db_actor::DbCommand,
    events::{ChangeSource, DomainEvent},
    identity::MediaId,
};
use std::{cell::Cell, rc::Rc, time::Duration};

// GTK binds to the thread that calls `gtk::init()`, and libtest gives every
// `#[test]` its own thread, so this binary carries a single scenario.
#[test]
fn a_local_import_keeps_existing_tiles_painted() {
    let _sandbox = photo_viewer::testing::Sandbox::install();
    gtk::init().unwrap();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let _guard = runtime.enter();
    let shell = Shell::with_photos_and_events(6);
    let grid = shell.visible_photos_grid();
    let target = MediaId::from(shell.items[3].id);
    let tile = shell.tile_for(&grid, target, "existing photo");
    // Let initial metadata/range work settle before watching for rebinds.
    shell.ui.wait_until(Duration::from_millis(300), || false);
    let picture = descendants::<gtk::Picture>(&tile)
        .into_iter()
        .find(|picture| picture.has_css_class("thumb-image"))
        .unwrap();
    let paintable = picture.paintable().expect("the existing photo is painted");
    let replacements = Rc::new(Cell::new(0));
    picture.connect_paintable_notify({
        let replacements = replacements.clone();
        move |_| replacements.set(replacements.get() + 1)
    });

    // An ordinary watcher discovery, not a download and not a bulk scan. It
    // lands in the same day section as the newest fixture, so the section tail
    // shifts while every other section must keep its painted objects.
    let path = write_photo(&shell.photos_dir(), "imported-locally", 4);
    let mut imported = LocalBackend::new_item_from_path(&path).unwrap().unwrap();
    imported.taken_at = Some(shell.items[0].sort_datetime() + chrono::Duration::hours(1));
    shell
        .db_actor
        .execute_blocking(DbCommand::UpsertMediaBatch {
            source: ChangeSource::FilesystemWatcher,
            items: vec![imported],
        })
        .unwrap();
    let mut inserted = None;
    assert!(
        shell.ui.wait_until(Duration::from_secs(5), || {
            while let Ok(event) = shell.db_events.as_ref().unwrap().borrow_mut().try_recv() {
                if let DomainEvent::MediaUpserted {
                    source: ChangeSource::FilesystemWatcher,
                    items,
                } = &event
                {
                    inserted = items.first().map(|item| item.id);
                    assert!(shell
                        .window
                        .apply_incremental_media_event(&event, &shell.media_list));
                }
            }
            inserted.is_some() && grid.logical_media_count() == 7
        }),
        "a local import must be admitted through the incremental grid path"
    );
    shell.ui.wait_until(Duration::from_millis(400), || false);

    assert_eq!(grid.logical_media_count(), 7);
    assert!(
        grid.tile_for_media(MediaId::from(inserted.unwrap()))
            .is_some(),
        "the imported photo must appear in the grid"
    );
    assert_eq!(
        grid.tile_for_media(target).unwrap(),
        tile,
        "a local import must not replace an unrelated tile"
    );
    assert_eq!(picture.paintable().unwrap(), paintable);
    assert_eq!(
        replacements.get(),
        0,
        "a local import must not reset or repaint existing thumbnails"
    );
}
