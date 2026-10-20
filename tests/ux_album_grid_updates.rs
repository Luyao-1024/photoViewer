//! A download into another album must not reload the album page being viewed.
mod common;
use common::{
    interaction::descendants,
    shell::{write_photo, Shell},
    sidebar,
};
use gtk::prelude::*;
use gtk4 as gtk;
use photo_viewer::core::{
    backend::local::LocalBackend,
    db_actor::DbCommand,
    events::{ChangeSource, DomainEvent},
    identity::MediaId,
};
use photo_viewer::ui::{virtual_media_grid::VirtualMediaGrid, AlbumDetailPage};
use std::{cell::Cell, rc::Rc, time::Duration};

// GTK binds to the thread that calls `gtk::init()`, and libtest gives every
// `#[test]` its own thread, so this binary carries a single scenario.
#[test]
fn a_download_into_another_album_does_not_reload_the_open_album() {
    let _sandbox = photo_viewer::testing::Sandbox::install();
    gtk::init().unwrap();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let _guard = runtime.enter();
    let shell = Shell::with_photos_and_events(6);
    sidebar::open_album(&shell, &shell.photos_dir());
    let detail = shell
        .window
        .browsing_stack()
        .visible_child()
        .and_downcast::<AlbumDetailPage>()
        .expect("the folder album must be open");
    let grid = descendants::<VirtualMediaGrid>(&detail)
        .into_iter()
        .next()
        .expect("the album page must own a grid");
    assert!(shell
        .ui
        .wait_until(Duration::from_secs(10), || grid.logical_media_count() == 6));
    let target = MediaId::from(shell.items[3].id);
    let tile = shell.tile_for(&grid, target, "an album photo");
    // Let initial metadata/range work settle before watching for rebinds.
    shell.ui.wait_until(Duration::from_millis(300), || false);
    let picture = descendants::<gtk::Picture>(&tile)
        .into_iter()
        .find(|picture| picture.has_css_class("thumb-image"))
        .unwrap();
    let paintable = picture.paintable().expect("the album photo is painted");
    let replacements = Rc::new(Cell::new(0));
    picture.connect_paintable_notify({
        let replacements = replacements.clone();
        move |_| replacements.set(replacements.get() + 1)
    });

    // A real actor-produced download event. Its file lives outside the open
    // album's folder, so this grid's query does not contain it.
    let elsewhere = shell.tmp.path().join("downloaded-elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    let path = write_photo(&elsewhere, "downloaded", 2);
    let mut downloaded = LocalBackend::new_item_from_path(&path).unwrap().unwrap();
    downloaded.taken_at = Some(shell.items[0].sort_datetime() + chrono::Duration::hours(2));
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
                        .apply_incremental_media_event(&event, &shell.media_list));
                    received = true;
                }
            }
            received
        }),
        "the download event must be routed"
    );
    shell.ui.wait_until(Duration::from_millis(600), || false);

    assert_eq!(
        grid.logical_media_count(),
        6,
        "a download into another album must not change this album's count"
    );
    assert_eq!(
        grid.tile_for_media(target).unwrap(),
        tile,
        "the open album's tile must retain its identity"
    );
    assert_eq!(picture.paintable().unwrap(), paintable);
    assert_eq!(
        replacements.get(),
        0,
        "a download into another album must not rebind this album's thumbnails"
    );

    // A cloud-only admission whose thumbnail is still in flight: the row is
    // real but `visible_media` excludes it, which is the shape every downlink
    // admission has before its thumbnail lands. The grid cannot show it, so it
    // must not force the open page to reload either.
    let cloud_dir = shell.tmp.path().join("cloud-inflight");
    std::fs::create_dir_all(&cloud_dir).unwrap();
    let cloud_path = cloud_dir.join("pending.jpg");
    let cloud_uri = photo_viewer::core::file_uri::from_path(&cloud_path);
    shell
        .pool
        .get()
        .unwrap()
        .execute(
            "INSERT INTO media_items(uri,path,folder_path,mime_type,file_mtime,file_size,blake3_hash,indexed_at,storage_location,thumbnail_ready)
             VALUES(?1,?2,?3,'image/jpeg',1,1,'',1,2,0)",
            rusqlite::params![
                cloud_uri,
                cloud_path.to_string_lossy(),
                cloud_dir.to_string_lossy()
            ],
        )
        .unwrap();
    let cloud_item = photo_viewer::core::db::get_media_item_by_uri(&shell.pool, &cloud_uri)
        .unwrap()
        .expect("the admitted cloud row must exist");
    assert!(
        !cloud_item.can_display(),
        "a cloud row without a thumbnail is not displayable"
    );
    let event = DomainEvent::MediaUpserted {
        source: ChangeSource::Synchronization,
        items: vec![cloud_item],
    };
    assert!(
        !shell
            .window
            .apply_incremental_media_event(&event, &shell.media_list),
        "an admission the grid cannot show must not enter the incremental path"
    );
    // This is the fall-through the app performs for unroutable media events.
    shell
        .window
        .refresh_visible_album_detail_page("media_event");
    shell.ui.wait_until(Duration::from_millis(800), || false);
    assert_eq!(grid.logical_media_count(), 6);
    assert_eq!(grid.tile_for_media(target).unwrap(), tile);
    assert_eq!(picture.paintable().unwrap(), paintable);
    assert_eq!(
        replacements.get(),
        0,
        "an admission without a thumbnail must not rebind the open album"
    );

    // The same admission must still insert a download that belongs to this
    // album, without rebinding the photos below it.
    let path = write_photo(&shell.photos_dir(), "downloaded-into-album", 3);
    let mut downloaded = LocalBackend::new_item_from_path(&path).unwrap().unwrap();
    downloaded.taken_at = Some(shell.items[0].sort_datetime() + chrono::Duration::hours(3));
    shell
        .db_actor
        .execute_blocking(DbCommand::UpsertMediaBatch {
            source: ChangeSource::Synchronization,
            items: vec![downloaded],
        })
        .unwrap();
    let mut applied = false;
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
                        .apply_incremental_media_event(&event, &shell.media_list));
                    applied = true;
                }
            }
            applied && grid.logical_media_count() == 7
        }),
        "a download into the open album must be inserted incrementally; applied={applied}, count={}",
        grid.logical_media_count()
    );
    shell.ui.wait_until(Duration::from_millis(400), || false);
    assert_eq!(grid.tile_for_media(target).unwrap(), tile);
    assert_eq!(picture.paintable().unwrap(), paintable);
    assert_eq!(
        replacements.get(),
        0,
        "inserting into the album must not rebind its untouched photos"
    );

    // The app refreshes the open album's filtered store after every media
    // event. That splice must reach the grid's incremental path instead of
    // being treated as an unknown ListStore change: the latter reloads the
    // whole metadata layout once per event (one reload every ~150 ms during
    // native sync), rebinding every painted tile.
    shell
        .window
        .refresh_visible_album_detail_page("media_updated");
    shell.ui.wait_until(Duration::from_millis(800), || false);
    assert_eq!(grid.logical_media_count(), 7);
    assert_eq!(
        grid.tile_for_media(target).unwrap(),
        tile,
        "the album store refresh must not rebuild the grid's tiles"
    );
    assert_eq!(picture.paintable().unwrap(), paintable);
    assert_eq!(
        replacements.get(),
        0,
        "the album store refresh must not rebind resident thumbnails"
    );
}
