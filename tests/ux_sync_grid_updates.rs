//! A background admission must leave unrelated, selected painted photos intact.
mod common;
use common::{
    interaction::{descendants, wait_for_button_with_label},
    shell::{write_photo, Shell},
};
use gtk::prelude::*;
use gtk4 as gtk;
use photo_viewer::core::{
    backend::local::LocalBackend,
    db_actor::DbCommand,
    events::{ChangeSource, DomainEvent},
    i18n::tr,
    identity::MediaId,
};
use std::{cell::Cell, rc::Rc, time::Duration};

#[test]
fn sync_download_keeps_existing_tiles_painted_and_selected() {
    let _sandbox = photo_viewer::testing::Sandbox::install();
    gtk::init().unwrap();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let _guard = runtime.enter();
    let shell = Shell::with_photos_and_events(6);
    let grid = shell.visible_photos_grid();
    let target = MediaId::from(shell.items[3].id);
    let tile = shell.tile_for(&grid, target, "existing photo");
    shell.ui.right_click(&tile, "select an existing photo");
    let select = wait_for_button_with_label(
        &shell.window,
        &tr("photos.batch.multi_select"),
        Duration::from_secs(5),
    )
    .unwrap();
    shell.ui.click(&select, "Multi select");
    assert!(shell.ui.wait_until(Duration::from_secs(5), || grid
        .selected_ids()
        .contains(&target)));
    // Let initial metadata/range work settle before watching for rebinds.
    shell.ui.wait_until(Duration::from_millis(300), || false);
    let picture = descendants::<gtk::Picture>(&tile)
        .into_iter()
        .find(|picture| picture.has_css_class("thumb-image"))
        .unwrap();
    let paintable = picture.paintable().expect("the selected photo is painted");
    let scroller = descendants::<gtk::ScrolledWindow>(&grid)
        .into_iter()
        .next()
        .unwrap();
    let scroll_before = scroller.vadjustment().value();
    assert!(
        scroll_before > 0.0,
        "the real input must scroll away from the beginning"
    );
    let replacements = Rc::new(Cell::new(0));
    picture.connect_paintable_notify({
        let replacements = replacements.clone();
        move |_| replacements.set(replacements.get() + 1)
    });

    // Deliver a real actor-produced background event for newly downloaded
    // bytes. Its older timestamp also exercises a UTC/local-date boundary
    // when run with TZ=Asia/Shanghai.
    let path = write_photo(&shell.photos_dir(), "downloaded", 2);
    let mut downloaded = LocalBackend::new_item_from_path(&path).unwrap().unwrap();
    downloaded.taken_at = Some(shell.items[5].sort_datetime() - chrono::Duration::hours(18));
    shell
        .db_actor
        .execute_blocking(DbCommand::UpsertMediaBatch {
            source: ChangeSource::Synchronization,
            items: vec![downloaded],
        })
        .unwrap();
    let mut received = false;
    assert!(
        shell.ui.wait_until(Duration::from_secs(5), || {
            while let Ok(event) = shell.db_events.as_ref().unwrap().borrow_mut().try_recv() {
                if matches!(
                    &event,
                    DomainEvent::MediaUpserted {
                        source: ChangeSource::Synchronization,
                        ..
                    }
                ) {
                    assert!(shell
                        .window
                        .apply_synchronization_media_event(&event, &shell.media_list));
                    received = true;
                }
            }
            received && grid.logical_media_count() == 7
        }),
        "the real admission event must insert the downloaded photo"
    );
    shell.ui.wait_until(Duration::from_millis(400), || false);
    assert!(
        (scroller.vadjustment().value() - scroll_before).abs() < 1.0,
        "a later photo must not reset the user's scroll position: before={scroll_before}, after={}",
        scroller.vadjustment().value()
    );

    // An availability notification for an already visible row must also avoid
    // treating the row as a new insertion or replacing the whole layout.
    let existing = photo_viewer::core::db::get_media_item(&shell.pool, shell.items[0].id).unwrap();
    let update = DomainEvent::MediaUpdated {
        source: ChangeSource::Synchronization,
        items: vec![existing],
        fields: photo_viewer::core::events::MediaFields {
            favorite: false,
            metadata: false,
            location: true,
            trash: false,
            thumbnail: true,
            attributes: false,
        },
    };
    assert!(shell
        .window
        .apply_synchronization_media_event(&update, &shell.media_list));
    shell.ui.wait_until(Duration::from_millis(400), || false);
    assert_eq!(grid.logical_media_count(), 7);
    let target_y = tile.compute_bounds(&grid).unwrap().y();
    let newer_path = write_photo(&shell.photos_dir(), "newer-download", 3);
    let mut newer = LocalBackend::new_item_from_path(&newer_path)
        .unwrap()
        .unwrap();
    newer.taken_at = Some(shell.items[0].sort_datetime() + chrono::Duration::days(1));
    shell
        .db_actor
        .execute_blocking(DbCommand::UpsertMediaBatch {
            source: ChangeSource::Synchronization,
            items: vec![newer],
        })
        .unwrap();
    assert!(shell.ui.wait_until(Duration::from_secs(5), || {
        while let Ok(event) = shell.db_events.as_ref().unwrap().borrow_mut().try_recv() {
            if matches!(
                &event,
                DomainEvent::MediaUpserted {
                    source: ChangeSource::Synchronization,
                    ..
                }
            ) {
                assert!(shell
                    .window
                    .apply_synchronization_media_event(&event, &shell.media_list));
            }
        }
        grid.logical_media_count() == 8
    }));
    shell.ui.wait_until(Duration::from_millis(400), || false);
    assert!(
        (tile.compute_bounds(&grid).unwrap().y() - target_y).abs() < 2.0,
        "inserting a newer date must keep the same photo at the same viewport position"
    );
    assert_eq!(
        grid.tile_for_media(target).unwrap(),
        tile,
        "the existing tile must retain its identity"
    );
    assert_eq!(picture.paintable().unwrap(), paintable);
    assert_eq!(
        replacements.get(),
        0,
        "another photo's download must not reset or repaint this thumbnail"
    );
    assert!(grid.selected_ids().contains(&target));
}
