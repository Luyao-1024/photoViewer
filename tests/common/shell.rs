//! The full application shell used by UX scenarios: a real `MainWindow` with a
//! real database, real decodable photos on a real filesystem, and the production
//! actors wired the way `src/main.rs` wires them.
//!
//! Scenarios start here and drive it through [`super::interaction::Ui`], so a
//! journey crosses the same page boundaries, persistence layer, and filesystem
//! effects a user does. Nothing is pre-seeded into a widget's final state: the
//! shell is built, then interacted with.
//!
//! Two constraints shape this fixture and are worth keeping in mind when copying
//! it:
//!
//! - **On a trash-capable filesystem.** `gio` refuses to trash files on some
//!   tmpfs mounts, so the library lives under `$HOME`. Pre-seeding database state
//!   to dodge that would skip the real GIO trash/restore behaviour, which is part
//!   of what the journeys assert.
//! - **A unique application id per shell.** `GApplication` is single-instance per
//!   id per process and every scenario builds its own shell, so ids come from a
//!   monotonic counter.

use super::interaction::Ui;
use chrono::{TimeZone, Utc};
use gtk4 as gtk;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::subclass::prelude::ObjectSubclassIsExt;
use image::{ImageBuffer, Rgb};
use libadwaita as adw;
use photo_viewer::core::media::{MediaItem, NewMediaItem, MEDIA_SUBKIND_STANDARD};
use photo_viewer::core::thumbnails::ThumbnailLoader;
use photo_viewer::core::{albums, db};
use photo_viewer::ui::virtual_media_grid::VirtualMediaGrid;
use photo_viewer::ui::{MainWindow, PhotosPage, TrashPage};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

static SHELL_SEQ: AtomicU64 = AtomicU64::new(0);

/// A presented, realized application shell plus the library it shows.
///
/// Dropping the shell destroys the window: leaving several shells mapped
/// starves the frame clock, so later scenarios would depend on how many ran
/// before them.
pub struct Shell {
    pub app: adw::Application,
    pub tmp: tempfile::TempDir,
    pub pool: db::DbPool,
    pub loader: Arc<ThumbnailLoader>,
    pub media_list: gtk::gio::ListStore,
    pub db_actor: photo_viewer::core::db_actor::DbActorHandle,
    pub items: Vec<MediaItem>,
    pub window: MainWindow,
    pub photos: PhotosPage,
    pub ui: Ui,
}

impl Drop for Shell {
    fn drop(&mut self) {
        self.window.destroy();
    }
}

impl Shell {
    /// Build the shell with `photo-0.jpg .. photo-{count-1}.jpg`, each a real
    /// distinct JPEG with its own size, colour and capture day, so a switch is
    /// observable in the pixels, in the header date, and by file identity.
    pub fn with_photos(count: usize) -> Self {
        build(count)
    }

    /// The default fixture: enough photos to exercise selection, batching, and
    /// a multi-item trash journey.
    pub fn new() -> Self {
        build(2)
    }

    /// The tile currently showing `media_id`, waiting for the grid to realize it.
    ///
    /// A pointer has to be aimed at a widget, not at a slot number: virtualization
    /// recycles item widgets, so "the photo called one.jpg" is the only stable
    /// thing a scenario can aim at.
    pub fn tile_for(
        &self,
        grid: &VirtualMediaGrid,
        media_id: photo_viewer::core::identity::MediaId,
        label: &str,
    ) -> photo_viewer::ui::SquareTile {
        let mut found: Option<photo_viewer::ui::SquareTile> = None;
        let reached = self.ui.wait_until(Duration::from_secs(10), || {
            found = grid.tile_for_media(media_id);
            found.is_some()
        });
        assert!(
            reached,
            "{label} should have realized a tile for media {} within 10s",
            media_id.get()
        );
        let tile = found.expect("the tile was captured above");
        // A realized tile is not necessarily a painted one: the thumbnail is
        // decoded off-thread, and aiming a pointer at a tile that has no picture
        // yet measures a layout the user never saw.
        assert!(
            self.ui
                .wait_until(Duration::from_secs(10), || tile.full_thumbnail_painted()
                    || tile.cache_key().is_some()),
            "the tile for media {} should paint a thumbnail before it is clicked",
            media_id.get()
        );
        tile
    }

    /// Open the Trash page the way a user does: click its sidebar row. Returns
    /// the `TrashPage` the window built from its own pool, loader and actors.
    pub fn open_trash(&self) -> TrashPage {
        let rows = self.window.imp().trash_list.get();
        let row = rows.row_at_index(0).expect("the sidebar has a Trash row");
        self.ui.click(&row, "Trash sidebar row");
        assert!(
            self.ui.wait_until(Duration::from_secs(3), || {
                self.window
                    .nav_view()
                    .visible_page()
                    .and_downcast::<TrashPage>()
                    .is_some()
            }),
            "clicking the Trash sidebar row should show TrashPage"
        );
        self.window
            .nav_view()
            .visible_page()
            .and_downcast::<TrashPage>()
            .expect("TrashPage is visible")
    }

    /// The Photos grid that is currently visible — whichever mode the page is on.
    pub fn visible_photos_grid(&self) -> VirtualMediaGrid {
        let stack = super::interaction::find_descendant::<gtk::Stack>(&self.photos)
            .expect("PhotosPage should contain a GtkStack");
        stack
            .visible_child()
            .and_downcast::<VirtualMediaGrid>()
            .expect("the active Photos mode should use VirtualMediaGrid")
    }

    /// Add a second real album directory, `second-album/three.jpg`, so album
    /// scenarios have more than the one folder album the library implies.
    pub fn seed_extra_album(&self) -> PathBuf {
        let album_dir = self.tmp.path().join("second-album");
        std::fs::create_dir_all(&album_dir).unwrap();
        let path = write_photo(&album_dir, "three", 3);
        let item = sample_item(
            i64::try_from(self.items.len()).unwrap_or(0) + 200,
            path,
            200,
        );
        super::db::insert_media_item(&self.pool, &NewMediaItem::from(&item)).unwrap();
        albums::refresh(&self.pool).unwrap();
        self.window.populate_album_rows();
        album_dir
    }
}

fn build(photo_count: usize) -> Shell {
    let fixture_root = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/var/tmp"));
    let tmp = tempfile::Builder::new()
        .prefix("photo-viewer-shell-")
        .tempdir_in(fixture_root)
        .expect("create the full-shell fixture on a trash-capable filesystem");
    let pool = db::init_pool(&tmp.path().join("shell.db")).unwrap();
    let loader = Arc::new(ThumbnailLoader::new(
        pool.clone(),
        tmp.path().join("thumbs"),
    ));
    let items = seed_media(&pool, tmp.path(), photo_count);
    albums::refresh(&pool).unwrap();

    let media_list = gtk::gio::ListStore::new::<glib::BoxedAnyObject>();
    for item in &items {
        media_list.append(&glib::BoxedAnyObject::new(item.clone()));
    }
    let (event_sender, _event_rx) = photo_viewer::core::DomainEventSender::new();
    let db_actor = photo_viewer::core::start_db_actor(pool.clone(), event_sender);

    let seq = SHELL_SEQ.fetch_add(1, Ordering::Relaxed);
    let app = adw::Application::builder()
        .application_id(format!("io.github.luyao_1024.photoviewer.Shell{seq}"))
        .build();
    app.register(None::<&gtk::gio::Cancellable>)
        .expect("the test application should register");
    photo_viewer::ui::grid_css::install();

    let window = MainWindow::new(&app);
    window.populate_sidebar();
    window.set_resources(pool.clone(), loader.clone(), media_list.clone());
    window.set_db_actor(db_actor.clone());
    window.populate_album_rows();

    let nav = window.nav_view();
    let photos = PhotosPage::new(media_list.clone(), loader.clone());
    photos.set_nav_target(&nav);
    photos.set_db_pool(pool.clone());
    photos.set_db_actor(db_actor.clone());
    window.show_photos_browsing_page(&photos);
    window.connect_sidebar(&nav);

    // A desktop user gets a desktop-sized window. The app derives its default
    // width from the Day-grid column preference, which under a small headless
    // screen collapses to a narrow layout the breakpoint rules then restyle —
    // tiles, sidebar and chrome all land somewhere a real pointer could not
    // reach. Fixtures therefore claim a normal window instead.
    window.set_default_size(1_440, 900);
    window.maximize();

    // Present, and wait for the page to map. Production code gates on widget
    // visibility — `PhotosPage::open_viewer` bails when the browsing root is not
    // shown — so an un-presented window would make those guards take the other
    // branch and the scenario would drive a shell no user ever sees.
    window.present();
    let ui = Ui::for_window(&window);
    assert!(
        ui.wait_until(Duration::from_secs(15), || photos.is_visible()
            && photos.is_mapped()),
        "the full app shell failed to realize the browsing page within 15s"
    );
    ui.pump(Duration::from_millis(200));
    // Every contract in a UX scenario is a pointer position, and a pointer
    // position only exists inside what the display actually shows. xvfb-run's
    // default screen is 640x480, which clamps the window, trips the layout's
    // breakpoints and puts controls outside the surface — so say so here rather
    // than reporting an unreachable control as a UI bug. Run the suite on a
    // desktop-sized virtual display, which is what CI now asks xvfb-run for.
    let (w, h) = (window.width(), window.height());
    assert!(
        w >= 1_000 && h >= 700,
        "the fixture window is {w}x{h}, too small for pointer-targeted UX tests: \
         controls fall outside the surface and the layout switches to its narrow \
         breakpoints. Start the display with `xvfb-run -a -s \"-screen 0 1920x1080x24\"`."
    );

    Shell {
        app,
        tmp,
        pool,
        loader,
        media_list,
        db_actor,
        items,
        window,
        photos,
        ui,
    }
}

/// Write `photo-N.jpg` files with distinct pixels, colours and capture days, and
/// insert them the way the scanner would.
fn seed_media(pool: &db::DbPool, root: &Path, count: usize) -> Vec<MediaItem> {
    let media_dir = root.join("photos");
    std::fs::create_dir_all(&media_dir).unwrap();
    let mut items = Vec::new();
    for idx in 0..count {
        let path = write_photo(&media_dir, &format!("photo-{idx}"), idx);
        let item = sample_item(i64::try_from(idx).unwrap(), path, idx);
        let id = super::db::insert_media_item(pool, &NewMediaItem::from(&item)).unwrap();
        items.push(db::get_media_item(pool, id).unwrap());
    }
    items
}

/// A real, decodable JPEG with its own size and colour.
pub fn write_photo(dir: &Path, stem: &str, idx: usize) -> PathBuf {
    let size = 48 + 16 * (idx as u32);
    let base = [
        30u32 + 55 * (idx as u32 % 3),
        80u32 + 45 * (idx as u32 % 2),
        170u32 - 35 * (idx as u32 % 4),
    ];
    let img = ImageBuffer::<Rgb<u8>, _>::from_fn(size, size, |x, y| {
        Rgb([
            (base[0] + x % 32) as u8,
            (base[1] + y % 32) as u8,
            (base[2] - (x + y) % 24) as u8,
        ])
    });
    let path = dir.join(format!("{stem}.jpg"));
    img.save(&path).expect("write a sample photo");
    path
}

/// One distinct capture day per photo, newest first, the way the library sorts
/// (`COALESCE(taken_at, file_mtime) DESC`). Kept well away from today so the
/// viewer header renders a real date rather than the localized 今天/昨天 shortcut.
pub fn sample_item(id: i64, path: PathBuf, day_shift: usize) -> MediaItem {
    // One distinct day per photo, descending: photo-0 is the newest and therefore
    // rank 1, the way the library sorts (`COALESCE(taken_at, file_mtime) DESC`).
    // Kept inside June so a large fixture cannot underflow the day.
    let day = 28u32 - (day_shift as u32) % 28;
    let taken_at = Utc.with_ymd_and_hms(2026, 6, day, 12, 0, 0).unwrap();
    let folder_path = path
        .parent()
        .unwrap_or_else(|| Path::new("/tmp"))
        .to_path_buf();
    MediaItem {
        id,
        uri: format!("file://{}", path.display()),
        path,
        folder_path,
        mime_type: "image/jpeg".into(),
        media_subkind: MEDIA_SUBKIND_STANDARD.into(),
        media_attributes: "{}".into(),
        width: Some(64),
        height: Some(64),
        video_duration_secs: None,
        taken_at: Some(taken_at),
        file_mtime: taken_at,
        file_size: 128,
        blake3_hash: format!("shell-hash-{id}"),
        is_favorite: false,
        trashed_at: None,
    }
}
