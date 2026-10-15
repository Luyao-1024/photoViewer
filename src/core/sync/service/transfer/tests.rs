//! What happens between "bytes arrived" and "photo is visible".
//!
//! These tests run the publication path without a server: the staging,
//! publishing and admission steps are all local, and they are where the
//! half-pair failure would appear. The network half is covered by
//! `tests/native_sync_assets.rs`.

use super::*;
use crate::core::sync::store::tests::{new_job, temp_root};
use crate::core::sync::AssetDescriptor;
use crate::core::sync::SyncJob;

const GENERATION: &str = "501855bb-18d3-4008-abc3-37e4948bf6f0";

struct Fixture {
    service: SyncService,
    job: SyncJob,
    local_root: PathBuf,
}

fn fixture(name: &str) -> Fixture {
    let dir = temp_root(&format!("xfer-{name}"));
    let pool = crate::core::db::init_pool(&dir.join("library.db")).unwrap();
    // The staging root is this fixture's own directory, never the user's real
    // one: tests staging the same asset id would otherwise see each other's
    // bytes and "verify" work neither of them did.
    let service = SyncService::with_staging_root_for_tests(pool.clone(), dir.join("staging"));
    let local_root = temp_root(&format!("xfer-{name}-library"));
    let job = service
        .store()
        .create_job(&new_job(local_root.clone()))
        .unwrap();
    service
        .store()
        .begin_native_receive(
            job.id,
            crate::core::sync::native::demo_pinned_scope("https://s.invalid/", &["PrimarySync"])
                .namespace,
            GENERATION,
            "scope-1",
            crate::core::sync::NativeZone::new("PrimarySync"),
        )
        .unwrap();
    Fixture {
        service,
        job,
        local_root,
    }
}

fn descriptor(asset_id: &str, size: u64) -> AssetDescriptor {
    AssetDescriptor {
        asset_id: asset_id.into(),
        size,
        file_checksum: "47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU=".into(),
        mime_type: "image/jpeg".into(),
        width: Some(800),
        height: Some(600),
    }
}

fn photo(photo_id: &str, original: &str, thumbnail: &str) -> crate::core::sync::RemotePhoto {
    crate::core::sync::RemotePhoto {
        photo_id: photo_id.into(),
        master_id: format!("master-{photo_id}"),
        content_generation: "gen-1".into(),
        relative_path: format!("camera/{photo_id}.jpg"),
        is_favorite: true,
        asset_date_ms: Some(1_700_000_000_000),
        is_hidden: false,
        original: descriptor(original, 2048),
        thumbnail: descriptor(thumbnail, 512),
        thumbnail_source_sha256: "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
            .into(),
        events: Default::default(),
    }
}

fn promote(fixture: &Fixture, photos: Vec<crate::core::sync::RemotePhoto>) {
    fixture
        .service
        .store()
        .promote_remote_graph(
            fixture.job.id,
            crate::core::sync::RemoteGraph {
                photos,
                records: Vec::new(),
                tombstones: Vec::new(),
            },
            "zone.target",
        )
        .unwrap();
}

fn jpeg_bytes(width: i32, height: i32) -> Vec<u8> {
    let image = gdk_pixbuf::Pixbuf::new(gdk_pixbuf::Colorspace::Rgb, false, 8, width, height)
        .expect("build fixture");
    image.fill(0x77AA33);
    image.save_to_bufferv("jpeg", &[]).expect("encode")
}

#[test]
fn verified_bytes_are_staged_under_an_owned_name_not_a_url() {
    let fixture = fixture("stage-name");
    let bytes = jpeg_bytes(64, 48);
    let path = fixture
        .service
        .stage_verified("asset-original-1", AssetRole::Original, &bytes)
        .unwrap();
    assert!(path.is_file());
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    assert!(path.starts_with(fixture.service.staging_root()));

    // The name is the asset id, never a remote URL: a URL is re-minted on every
    // describe and would leave an unbounded pile of "owned" files behind.
    let name = path.file_name().unwrap().to_string_lossy().to_string();
    assert!(!name.contains("http"), "{name}");
    assert!(!name.contains('?'), "{name}");
}

#[test]
fn a_hostile_asset_id_cannot_escape_the_staging_directory() {
    let fixture = fixture("hostile-id");
    let path = fixture
        .service
        .stage_verified("../../etc/passwd", AssetRole::Original, b"x")
        .unwrap();
    assert!(path.starts_with(fixture.service.staging_root()));
    assert!(!path.to_string_lossy().contains(".."), "{path:?}");
    assert_eq!(sanitize_asset_id(""), "unnamed");
}

#[test]
fn a_photo_with_only_its_original_verified_is_never_admitted() {
    let fixture = fixture("half-pair");
    let projected = photo("a1", "asset-original", "asset-thumb");
    promote(&fixture, vec![projected.clone()]);
    fixture
        .service
        .store()
        .begin_publication(
            fixture.job.id,
            None,
            "pub-a1",
            "a1",
            Some(&projected.master_id),
            Some(&projected.original.asset_id),
            Some(&projected.thumbnail.asset_id),
        )
        .unwrap();

    let original = fixture
        .service
        .stage_verified("asset-original", AssetRole::Original, &jpeg_bytes(64, 48))
        .unwrap();
    fixture
        .service
        .store()
        .verify_resource(
            fixture.job.id,
            "asset-original",
            "original",
            &original.to_string_lossy(),
        )
        .unwrap();

    let mut report = TransferReport::default();
    fixture
        .service
        .admit_complete_publications(fixture.job.id, &mut report)
        .unwrap();

    // The whole point: a visible photo with a missing original.
    assert_eq!(report.admitted, 0);
    assert!(
        !fixture.local_root.join("camera/a1.jpg").exists(),
        "a half-published pair must not reach the library"
    );
    assert_eq!(
        fixture
            .service
            .pool()
            .get()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM media_items", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
        0,
        "a half-published pair must not become a media row"
    );
}

#[test]
fn a_fully_verified_pair_is_published_and_admitted_exactly_once() {
    let fixture = fixture("complete-pair");
    let projected = photo("a1", "asset-original", "asset-thumb");
    promote(&fixture, vec![projected.clone()]);
    fixture
        .service
        .store()
        .begin_publication(
            fixture.job.id,
            None,
            "pub-a1",
            "a1",
            Some(&projected.master_id),
            Some(&projected.original.asset_id),
            Some(&projected.thumbnail.asset_id),
        )
        .unwrap();

    let original = fixture
        .service
        .stage_verified("asset-original", AssetRole::Original, &jpeg_bytes(800, 600))
        .unwrap();
    let thumbnail = fixture
        .service
        .stage_verified("asset-thumb", AssetRole::Thumbnail, &jpeg_bytes(200, 150))
        .unwrap();
    for (asset_id, role) in [
        ("asset-original", AssetRole::Original),
        ("asset-thumb", AssetRole::Thumbnail),
    ] {
        fixture
            .service
            .store()
            .verify_resource(fixture.job.id, asset_id, role.as_str(), "/cache/t.jpg")
            .unwrap();
    }

    let mut report = TransferReport::default();
    fixture
        .service
        .admit_complete_publications(fixture.job.id, &mut report)
        .unwrap();
    assert_eq!(report.admitted, 1);
    assert!(report.failures.is_empty(), "{:?}", report.failures);

    let published = fixture.local_root.join("camera/a1.jpg");
    assert!(published.is_file(), "the original must be in the job root");
    assert!(!published.to_string_lossy().contains(".photoviewer-sync-"));
    let rows: i64 = fixture
        .service
        .pool()
        .get()
        .unwrap()
        .query_row("SELECT COUNT(*) FROM media_items", [], |row| row.get(0))
        .unwrap();
    assert_eq!(rows, 1);

    // Running again admits nothing further: the publication is complete and the
    // row already exists, so the scanner and this path cannot fight.
    let mut second = TransferReport::default();
    fixture
        .service
        .admit_complete_publications(fixture.job.id, &mut second)
        .unwrap();
    assert_eq!(second.admitted, 0);
    let rows: i64 = fixture
        .service
        .pool()
        .get()
        .unwrap()
        .query_row("SELECT COUNT(*) FROM media_items", [], |row| row.get(0))
        .unwrap();
    assert_eq!(rows, 1, "admission must happen exactly once");
    let _ = (original, thumbnail);
}

#[test]
fn an_existing_file_is_replaced_only_after_the_new_bytes_are_in_place() {
    let fixture = fixture("replace");
    let projected = photo("a1", "asset-original", "asset-thumb");
    promote(&fixture, vec![projected.clone()]);

    let destination = fixture.local_root.join("camera/a1.jpg");
    std::fs::create_dir_all(destination.parent().unwrap()).unwrap();
    std::fs::write(&destination, b"the previous complete version").unwrap();

    fixture
        .service
        .store()
        .begin_publication(
            fixture.job.id,
            None,
            "pub-a1",
            "a1",
            Some(&projected.master_id),
            Some(&projected.original.asset_id),
            Some(&projected.thumbnail.asset_id),
        )
        .unwrap();
    fixture
        .service
        .stage_verified("asset-original", AssetRole::Original, &jpeg_bytes(800, 600))
        .unwrap();
    fixture
        .service
        .stage_verified("asset-thumb", AssetRole::Thumbnail, &jpeg_bytes(200, 150))
        .unwrap();
    for (asset_id, role) in [
        ("asset-original", AssetRole::Original),
        ("asset-thumb", AssetRole::Thumbnail),
    ] {
        fixture
            .service
            .store()
            .verify_resource(fixture.job.id, asset_id, role.as_str(), "/cache/t.jpg")
            .unwrap();
    }

    let mut report = TransferReport::default();
    fixture
        .service
        .admit_complete_publications(fixture.job.id, &mut report)
        .unwrap();
    assert_eq!(report.admitted, 1);
    // The previous complete version was replaced, not appended to.
    let bytes = std::fs::read(&destination).unwrap();
    assert_ne!(bytes, b"the previous complete version");
    assert_eq!(&bytes[0..2], &[0xFF, 0xD8], "the published file is a JPEG");
}

#[test]
fn a_verified_role_whose_staged_bytes_vanished_is_reported_not_admitted() {
    let fixture = fixture("vanished");
    let projected = photo("a1", "asset-original", "asset-thumb");
    promote(&fixture, vec![projected.clone()]);
    fixture
        .service
        .store()
        .begin_publication(
            fixture.job.id,
            None,
            "pub-a1",
            "a1",
            Some(&projected.master_id),
            Some(&projected.original.asset_id),
            Some(&projected.thumbnail.asset_id),
        )
        .unwrap();
    for (asset_id, role) in [
        ("asset-original", AssetRole::Original),
        ("asset-thumb", AssetRole::Thumbnail),
    ] {
        fixture
            .service
            .store()
            .verify_resource(fixture.job.id, asset_id, role.as_str(), "/cache/t.jpg")
            .unwrap();
    }
    // Both roles were verified at some point, but the bytes are gone now — the
    // staging directory was cleared behind our back.
    let mut report = TransferReport::default();
    fixture
        .service
        .admit_complete_publications(fixture.job.id, &mut report)
        .unwrap();
    assert_eq!(report.admitted, 0);
    assert_eq!(report.failures.len(), 1, "{:?}", report.failures);
    assert!(report.failures[0].1.contains("no staged bytes"));

    // The intent is still pending, not closed: a failed admission must be
    // retried once the bytes exist again, never lost.
    assert_eq!(
        fixture
            .service
            .store()
            .pending_publications(fixture.job.id)
            .unwrap()
            .len(),
        1
    );

    // And when the bytes do exist, the same intent admits.
    fixture
        .service
        .stage_verified("asset-original", AssetRole::Original, &jpeg_bytes(64, 48))
        .unwrap();
    fixture
        .service
        .stage_verified("asset-thumb", AssetRole::Thumbnail, &jpeg_bytes(32, 24))
        .unwrap();
    let mut retry = TransferReport::default();
    fixture
        .service
        .admit_complete_publications(fixture.job.id, &mut retry)
        .unwrap();
    assert_eq!(retry.admitted, 1, "the retried intent must admit");
    assert!(retry.failures.is_empty(), "{:?}", retry.failures);
}

#[test]
fn a_photo_that_left_the_projection_is_not_admitted_from_a_stale_intent() {
    let fixture = fixture("stale-intent");
    let projected = photo("a1", "asset-original", "asset-thumb");
    promote(&fixture, vec![projected.clone()]);
    fixture
        .service
        .store()
        .begin_publication(
            fixture.job.id,
            None,
            "pub-a1",
            "a1",
            Some(&projected.master_id),
            Some(&projected.original.asset_id),
            Some(&projected.thumbnail.asset_id),
        )
        .unwrap();
    for (asset_id, role) in [
        ("asset-original", AssetRole::Original),
        ("asset-thumb", AssetRole::Thumbnail),
    ] {
        fixture
            .service
            .store()
            .verify_resource(fixture.job.id, asset_id, role.as_str(), "/cache/t.jpg")
            .unwrap();
    }
    // A later round saw the photo deleted. The old intent must not resurrect it.
    promote(&fixture, Vec::new());

    let mut report = TransferReport::default();
    fixture
        .service
        .admit_complete_publications(fixture.job.id, &mut report)
        .unwrap();
    assert_eq!(report.admitted, 0);
    assert!(report
        .failures
        .iter()
        .any(|(_, reason)| reason.contains("left the projection")));
}

#[test]
fn a_photo_already_deleted_on_disk_is_never_created_from_staging() {
    let fixture = fixture("deleted-on-disk");
    let projected = photo("a1", "asset-original", "asset-thumb");
    promote(&fixture, vec![projected]);
    fixture
        .service
        .store()
        .begin_publication(fixture.job.id, None, "pub-a1", "a1", None, None, None)
        .unwrap();
    for (asset_id, role) in [
        ("asset-original", AssetRole::Original),
        ("asset-thumb", AssetRole::Thumbnail),
    ] {
        fixture
            .service
            .store()
            .verify_resource(fixture.job.id, asset_id, role.as_str(), "/cache/t.jpg")
            .unwrap();
    }
    let mut report = TransferReport::default();
    fixture
        .service
        .admit_complete_publications(fixture.job.id, &mut report)
        .unwrap();
    assert_eq!(
        report.admitted, 0,
        "an intent without asset ids cannot publish"
    );
    assert!(!fixture.local_root.join("camera/a1.jpg").exists());
}
