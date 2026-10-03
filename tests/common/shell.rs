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

/// A work directory at a *fixed* path that removes itself on drop.
///
/// `tempfile::TempDir` names its directory randomly, which is the opposite of
/// what a UX scenario wants: a scenario that asserts "Show in File Manager hands
/// over the folder the photo is in" needs a folder name it can name in the test.
/// Only the run root varies, so two concurrent runs never share a library; the
/// names and the layout below it are fixed and asserted by name.
pub struct TempDirIn {
    path: PathBuf,
}

impl TempDirIn {
    pub fn new(path: PathBuf) -> Self {
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create the fixed UX work directory");
        Self { path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDirIn {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// The committed fixture images a UX scenario lays out on disk, in the order
/// `Shell::with_photos` assigns them. They are in the repository rather than
/// generated per run, so a scenario's bytes are reviewable in a diff and two
/// machines see the same image.
///
/// Fourteen distinct scenes across **eight** aspect ratios, from 1:1 through
/// 4:3, 3:2 and 16:9 to 21:9 and 9:16. The spread is deliberate on both ends:
/// the filmstrip clamps anything past 21:9 horizontally or 9:21 vertically and
/// sizes each thumbnail from the media's real dimensions, so a single ratio can
/// only ever exercise one of those paths. Portrait and panorama fixtures are
/// here so the extremes are covered by the same journeys that use the middling
/// ones, rather than by a dedicated test that only someone remembering to extend
/// would run. It is also more than the strip's eleven-item window, so walking
/// the whole viewer crosses the window boundary and exercises the lazy
/// extension, not just the initial build.
///
/// Every scene is visually distinct at thumbnail size — the filmstrip is meant
/// to be readable at a glance, and a row of identical placeholder tiles cannot
/// be checked by eye, which is how a completely grey strip once passed.
const UX_FIXTURE_PHOTOS: [&str; 14] = [
    "ridge-square", // 420x420  1:1
    "lake-square",  // 380x380  1:1
    "coast-43",     // 480x360  4:3
    "forest-43",    // 512x384  4:3
    "mesa-32",      // 540x360  3:2
    "dunes-32",     // 600x400  3:2
    "harbour-169",  // 640x360  16:9
    "aurora-169",   // 672x378  16:9
    "pano-219",     // 840x360  21:9
    "fjord-219",    // 896x384  21:9
    "cliff-34",     // 360x480  3:4
    "willow-23",    // 400x600  2:3
    "night-916",    // 360x640  9:16
    "tower-916",    // 405x720  9:16
];

/// Read a JPEG's real pixel size out of its first start-of-frame marker.
///
/// The library row must agree with the file, and the cheapest way to guarantee
/// that is to stop declaring dimensions at all: this parses the bytes the
/// fixture actually has. A hand-written `width: Some(480)` beside a 512x384
/// image is a broken fixture, not a product bug, but it surfaces as one — a save
/// that rewrites the picture then fails to update what the row says about it.
///
/// Returns `None` for anything that is not a parseable JPEG, which is how the
/// undecodable fixture is expected to behave.
fn jpeg_dimensions(path: &Path) -> Option<(u32, u32)> {
    let bytes = std::fs::read(path).ok()?;
    // SOF0..SOF15, minus the markers that share the range and are not frame
    // headers (DHT, JPG and DAC).
    let mut i = 2usize; // skip SOI
    while i + 3 < bytes.len() {
        if bytes[i] != 0xFF {
            i += 1;
            continue;
        }
        let marker = bytes[i + 1];
        i += 2;
        match marker {
            0xD8 | 0x01 | 0xD0..=0xD7 => continue,
            0xD9 | 0xDA => return None, // EOI / start of scan
            _ => {}
        }
        if i + 1 >= bytes.len() {
            return None;
        }
        let segment_len = usize::from(u16::from_be_bytes([bytes[i], bytes[i + 1]]));
        let is_sof = (0xC0..=0xCF).contains(&marker) && !matches!(marker, 0xC4 | 0xC8 | 0xCC);
        if is_sof {
            if i + 7 > bytes.len() {
                return None;
            }
            let height = u32::from(u16::from_be_bytes([bytes[i + 3], bytes[i + 4]]));
            let width = u32::from(u16::from_be_bytes([bytes[i + 5], bytes[i + 6]]));
            return (width > 0 && height > 0).then_some((width, height));
        }
        i += segment_len;
    }
    None
}

/// The undecodable fixture: a `.jpg` that is actually a saved error page, which
/// is what a sync that wrote a 503 body, or an interrupted copy, leaves behind.
/// Committed so the media-error scenario tests a known file rather than bytes
/// spelled inline in a test body.
///
/// It has to be undecodable by *every* decoder, not merely damaged. A partially
/// corrupt JPEG still decodes to a partial image, and the viewer then
/// deliberately keeps showing that picture instead of the error surface — a
/// visible image must not be captioned "cannot be displayed". A truncated-but-
/// plausible JPEG therefore exercises the *wrong* branch, and lands in a stage
/// with a stale paintable and no error surface, which reads exactly like a
/// broken viewer.
const UX_FIXTURE_UNDECODABLE: &str = "undecodable.jpg";

/// Where a scenario's fixtures live relative to the shell root. The names are
/// part of the contract: the reveal journey asserts the handed-over folder is
/// `broken-album`, and `seed_extra_album` writes `second-album`.
const UX_DIR_PHOTOS: &str = "photos";
const UX_DIR_BROKEN: &str = "broken-album";
const UX_DIR_SECOND_ALBUM: &str = "second-album";

fn fixture_media_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("media")
        .join("ux")
}

/// Copy a committed fixture into `dir` under `name`, creating `dir` if needed.
fn install_fixture(dir: &Path, name: &str, fixture: &str) -> PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let source = if fixture.ends_with(".jpg") {
        fixture_media_dir().join(fixture)
    } else {
        fixture_media_dir().join(format!("{fixture}.jpg"))
    };
    let destination = dir.join(name);
    std::fs::copy(&source, &destination)
        .unwrap_or_else(|err| panic!("copy the {} fixture: {err}", source.display()));
    destination
}

/// A presented, realized application shell plus the library it shows.
///
/// Dropping the shell destroys the window: leaving several shells mapped
/// starves the frame clock, so later scenarios would depend on how many ran
/// before them.
pub struct Shell {
    pub app: adw::Application,
    pub tmp: TempDirIn,
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
        // Each shell starts its own thumbnail workers; ~27 scenarios would
        // otherwise leave that many detached threads decoding into a directory
        // that is about to be deleted.
        self.loader.shutdown();
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
        //
        // This waits for the paint alone. An earlier version accepted
        // `full_thumbnail_painted() || cache_key().is_some()`, and the cache key
        // is set the moment the tile is bound — so the disjunction was true
        // before anything decoded, and it quietly tolerated a shell whose
        // thumbnail workers were never started.
        assert!(
            self.ui
                .wait_until(Duration::from_secs(20), || tile.full_thumbnail_painted()),
            "the tile for media {} should paint a thumbnail before it is clicked",
            media_id.get()
        );
        tile
    }

    /// The indexed row installed from the named `UX_FIXTURE_PHOTOS` entry.
    ///
    /// A scenario whose expected geometry depends on the aspect ratio has to aim
    /// at a photo *of that ratio*: stepping the crop ring from "source" to 1:1
    /// only proves something when the source is not already square. Selecting by
    /// fixture name is what makes that dependency explicit, where indexing
    /// `items[0]` silently re-derives it from whatever the default two-photo
    /// library happens to sort first — which changed under this shell when the
    /// fixtures grew from uniform squares to a multi-ratio set, and broke a
    /// geometry assertion that had been quietly vacuous.
    pub fn item_from_fixture(&self, fixture: &str) -> MediaItem {
        let index = UX_FIXTURE_PHOTOS
            .iter()
            .position(|candidate| *candidate == fixture)
            .unwrap_or_else(|| panic!("{fixture} is not one of UX_FIXTURE_PHOTOS"));
        let name = format!("photo-{index}.jpg");
        self.items
            .iter()
            .find(|item| item.display_name() == name)
            .cloned()
            .unwrap_or_else(|| {
                panic!(
                    "the shell was built with fewer fixtures than {fixture} needs; \
                     ask for it with Shell::with_photos({})",
                    index + 1
                )
            })
    }

    /// Re-run the production library scan over the photos directory.
    ///
    /// This is `bootstrap::scan_and_aggregate_with_actor` — the same call the
    /// application's own "Retry Scan" and its startup scan make, with the same
    /// database actor, so the upserts reach the window through the real event
    /// path and the same pass also prunes rows whose files are gone.
    ///
    /// A scenario calls this directly rather than hunting for a button, because
    /// the only scan control the window exposes is the empty-state "Retry Scan",
    /// and that is unreachable once the library has anything in it. Driving the
    /// production function keeps the subject of the test the real scanner rather
    /// than a hand-built row, while still routing every effect through the
    /// actor and the UI's own event handling.
    ///
    /// Blocks on the async scan. The UX harness enters a Tokio runtime for
    /// exactly this reason.
    pub async fn rescan(&self) {
        photo_viewer::core::bootstrap::scan_and_aggregate_with_actor(
            &self.pool,
            &[self.photos_dir()],
            self.db_actor.clone(),
        )
        .await
        .expect("the production library scan should complete");
    }

    /// The directory the shell's photos live in — the library root a scan covers.
    pub fn photos_dir(&self) -> PathBuf {
        self.tmp.path().join(UX_DIR_PHOTOS)
    }

    /// Copy a committed fixture into the library as a *new* file, the way a user
    /// drops photos into a watched folder, and stamp it so the library's
    /// `COALESCE(taken_at, file_mtime) DESC` ordering is deterministic.
    ///
    /// Returns the installed path so a scenario can aim at the resulting row.
    pub fn import_fixture(&self, name: &str, fixture: &str, day_shift: usize) -> PathBuf {
        let path = install_fixture(&self.photos_dir(), name, fixture);
        set_fixture_mtime(&path, day_shift);
        path
    }

    /// Decode a real thumbnail for `item` through the production thumbnail
    /// loader, and report whether it produced an actual picture.
    ///
    /// This is the "and load" half of importing: the scanner finding a file is
    /// one thing, the application being able to render it is another, and only
    /// the second goes through the worker pool the grid and viewer use.
    pub async fn loads_thumbnail_for(&self, item: &MediaItem) -> bool {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.loader.request_for_media(
            item.id,
            item.uri.clone(),
            photo_viewer::core::thumbnails::ThumbnailSize::Medium,
            Some(item.file_mtime.into()),
            tx,
            photo_viewer::core::thumbnails::TIER_BOOST,
        );
        // Awaited rather than `blocking_recv`ed: the caller is already inside
        // the harness's Tokio runtime, and blocking a runtime thread on a
        // oneshot panics with "Cannot block the current thread from within a
        // runtime" — the loader's own workers keep ticking, so the reply still
        // arrives promptly.
        rx.await.map(|loaded| !loaded.unavailable).unwrap_or(false)
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

    /// Add a photo whose bytes are not a decodable image — what a truncated or
    /// corrupted file looks like to a user. The row scans and indexes normally,
    /// so the tile is a real tile a pointer can land on; only the viewer
    /// discovers the problem, when it tries to paint it and finds no paintable.
    ///
    /// This is deliberately a *file* problem rather than a missing row: a row
    /// that does not exist yet never reaches the viewer, and setting the error
    /// surface by hand would prove nothing about the path that produces it.
    ///
    /// Returns the inserted item, so the caller can aim at the tile by id —
    /// `Shell::tile_for` cannot be used here because it insists on a painted
    /// thumbnail, which is exactly what a corrupt file will never produce.
    pub fn seed_broken_photo(&self, stem: &str) -> MediaItem {
        let dir = self.tmp.path().join(UX_DIR_BROKEN);
        let path = install_fixture(&dir, &format!("{stem}.jpg"), UX_FIXTURE_UNDECODABLE);
        let mut item = sample_item(i64::try_from(self.items.len()).unwrap_or(0) + 500, path, 26);
        item.blake3_hash = format!("shell-broken-{stem}");
        let id = super::db::insert_media_item(&self.pool, &NewMediaItem::from(&item)).unwrap();
        item.id = id;
        self.media_list
            .append(&glib::BoxedAnyObject::new(item.clone()));
        item
    }

    /// Add a second real album directory, `second-album/three.jpg`, so album
    /// scenarios have more than the one folder album the library implies.
    pub fn seed_extra_album(&self) -> PathBuf {
        let album_dir = self.tmp.path().join(UX_DIR_SECOND_ALBUM);
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
    // The work root lives inside the project, under the gitignored `target/`, so
    // a failing run leaves its tree where a developer can look at it instead of
    // scattering directories through `$HOME`. Only the run root varies — the
    // directory names and the layout below it are fixed, and asserted by name,
    // because two concurrent runs must not share a library.
    let project_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let run_root = project_root
        .join("target")
        .join("ux-fixtures")
        .join(format!("run-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&run_root);
    std::fs::create_dir_all(&run_root).expect("create the UX fixture work root in the project");
    let tmp = TempDirIn::new(run_root);
    let pool = db::init_pool(&tmp.path().join("shell.db")).unwrap();
    let loader = Arc::new(ThumbnailLoader::new(
        pool.clone(),
        tmp.path().join("thumbs"),
    ));
    // The loader deliberately does not start its own workers — `src/app.rs`
    // calls `spawn_workers` at startup. Without this the whole suite runs
    // against a shell where no thumbnail ever decodes: grid tiles and the
    // viewer's filmstrip both stay blank, and every scenario that only checked
    // geometry or a tile's width sailed straight through. Production code gets
    // its workers from the app; a test shell has to ask for them too.
    loader.spawn_workers(photo_viewer::core::runtime_config::thumbnail_worker_count());
    let items = scan_media(&pool, tmp.path(), photo_count);
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

/// Give an installed fixture a deterministic modification time, newest first.
///
/// The scanner reads `file_mtime` off the filesystem, and the library sorts by
/// `COALESCE(taken_at, file_mtime) DESC`. Left to the copy, every fixture would
/// land within milliseconds of every other one and the ordering a scenario aims
/// at would be whatever SQLite happened to return. The dates also sit well away
/// from today so the viewer header renders a real date rather than the localized
/// 今天/昨天 shortcut.
fn set_fixture_mtime(path: &Path, index: usize) {
    let day = 28u32 - (index as u32) % 28;
    let when = std::time::SystemTime::from(Utc.with_ymd_and_hms(2026, 6, day, 12, 0, 0).unwrap());
    let file = std::fs::File::options()
        .write(true)
        .open(path)
        .expect("the installed fixture should be openable to stamp it");
    file.set_times(std::fs::FileTimes::new().set_modified(when))
        .expect("stamping a fixture's mtime should succeed");
}

/// Install the committed fixtures into a library directory and index them the
/// way the application does — `LocalBackend::scan_and_upsert_dir`, the same call
/// `bootstrap` makes at startup.
///
/// Nothing here builds a row by hand. A scenario therefore starts from files on
/// disk that the real scanner had to find, decode and index, which is the only
/// way "the library loaded my photos" is ever actually tested: a hand-built row
/// proves nothing about scanning, and the previous hand-built fixture is what
/// let a shell with no thumbnail workers at all look healthy.
///
/// Every indexed row is checked against the bytes before it is handed back, so a
/// fixture whose declared shape drifts from its file fails here rather than
/// surfacing later as a puzzling save-path failure.
fn scan_media(pool: &db::DbPool, root: &Path, count: usize) -> Vec<MediaItem> {
    let media_dir = root.join(UX_DIR_PHOTOS);
    std::fs::create_dir_all(&media_dir).unwrap();
    assert!(
        count <= UX_FIXTURE_PHOTOS.len(),
        "{count} photos were requested but only {} are committed under tests/fixtures/media/ux; \
         add the fixture rather than generating one, so the bytes stay reviewable",
        UX_FIXTURE_PHOTOS.len()
    );

    let mut expected: Vec<(PathBuf, (u32, u32))> = Vec::new();
    for (idx, fixture) in UX_FIXTURE_PHOTOS.iter().take(count).enumerate() {
        let path = install_fixture(&media_dir, &format!("photo-{idx}.jpg"), fixture);
        let dimensions = jpeg_dimensions(&path)
            .unwrap_or_else(|| panic!("{} is not a readable JPEG", path.display()));
        set_fixture_mtime(&path, idx);
        expected.push((path, dimensions));
    }

    let backend = photo_viewer::core::backend::local::LocalBackend::new(pool.clone());
    let upserted = backend
        .scan_and_upsert_dir(&media_dir)
        .expect("the real scanner should index the fixture library");
    assert_eq!(
        upserted,
        expected.len(),
        "the scanner should index every installed fixture: {upserted} of {}",
        expected.len()
    );

    let items = db::list_all_media(pool).expect("read the scanned library");
    assert_eq!(
        items.len(),
        expected.len(),
        "the library should hold exactly the installed fixtures"
    );

    // The scan is only "successful" if each row agrees with its file, so this is
    // where a shape mismatch is caught.
    for (path, (width, height)) in &expected {
        let row = items
            .iter()
            .find(|item| item.path == *path)
            .unwrap_or_else(|| panic!("the scan did not index {}", path.display()));
        assert_eq!(
            (row.width, row.height),
            (Some(*width), Some(*height)),
            "the row for {} disagrees with the bytes on disk",
            path.display()
        );
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
    let dimensions = jpeg_dimensions(&path);
    sample_item_with(id, path, day_shift, dimensions)
}

/// As [`sample_item`], with the pixel size stated outright. Scenarios that
/// install a *generated* image pass what they know; committed fixtures go
/// through [`sample_item`], which reads it from the file.
pub fn sample_item_with(
    id: i64,
    path: PathBuf,
    day_shift: usize,
    dimensions: Option<(u32, u32)>,
) -> MediaItem {
    // One distinct day per photo, descending: photo-0 is the newest and therefore
    // rank 1, the way the library sorts (`COALESCE(taken_at, file_mtime) DESC`).
    // Kept inside June so a large fixture cannot underflow the day.
    let day = 28u32 - (day_shift as u32) % 28;
    let taken_at = Utc.with_ymd_and_hms(2026, 6, day, 12, 0, 0).unwrap();
    // Read the size before `path` moves into the row below.
    let file_size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(128);
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
        // Parsed from the file, never declared. The library row has to agree
        // with the bytes on disk, and a hand-written size beside a differently
        // shaped image is a broken fixture that surfaces as a product bug: a
        // save that rewrites the picture then fails to update what the row says
        // about it.
        width: dimensions.map(|d| d.0),
        height: dimensions.map(|d| d.1),
        video_duration_secs: None,
        taken_at: Some(taken_at),
        file_mtime: taken_at,
        file_size,
        blake3_hash: format!("shell-hash-{id}"),
        is_favorite: false,
        trashed_at: None,
    }
}
