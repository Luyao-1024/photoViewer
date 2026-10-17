use super::*;
use crate::core::sync::{
    content::ContentDigest,
    native::records::{AssetDescriptor, RemotePhoto},
    store::tests::new_job,
    SyncJob, UploadScope,
};

struct Fixture {
    _temp: tempfile::TempDir,
    service: SyncService,
    job: SyncJob,
    root: PathBuf,
}
fn fixture() -> Fixture {
    crate::testing::Sandbox::install();
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("library");
    std::fs::create_dir_all(root.join("album")).unwrap();
    let pool = crate::core::db::init_pool(&temp.path().join("library.db")).unwrap();
    let service = SyncService::with_staging_root_for_tests(pool, temp.path().join("staging"));
    let mut config = new_job(root.clone());
    config.upload_scope = UploadScope::SelectedAlbums;
    config.upload_albums.clear();
    let job = service.store().create_job(&config).unwrap();
    service
        .store()
        .begin_native_receive(
            job.id,
            crate::core::sync::native::demo_pinned_scope(
                "https://sync.example.invalid/",
                &["PrimarySync"],
            )
            .namespace,
            "generation",
            "scope",
            crate::core::sync::NativeZone::new("PrimarySync"),
        )
        .unwrap();
    service
        .store()
        .pin_native_snapshot(job.id, "fixture.snapshot", "fixture.zone")
        .unwrap();
    for kind in crate::core::sync::native::receive::SUPPORTED_RECORD_TYPES {
        service
            .store()
            .save_bootstrap_page(job.id, kind, None, 0, "fixture.zone")
            .unwrap();
    }
    Fixture {
        _temp: temp,
        service,
        job,
        root,
    }
}
fn jpeg(color: [u8; 3]) -> Vec<u8> {
    let mut bytes = std::io::Cursor::new(Vec::new());
    image::RgbImage::from_pixel(32, 24, image::Rgb(color))
        .write_to(&mut bytes, image::ImageFormat::Jpeg)
        .unwrap();
    bytes.into_inner()
}
fn graph(bytes: &[u8]) -> RemoteGraph {
    let digest = ContentDigest::of(bytes);
    let asset = |name: &str| AssetDescriptor {
        asset_id: name.into(),
        size: digest.size,
        file_checksum: digest.base64.clone(),
        mime_type: "image/jpeg".into(),
        width: Some(32),
        height: Some(24),
    };
    RemoteGraph {
        photos: vec![RemotePhoto {
            photo_id: "cloud-photo".into(),
            master_id: "cloud-master".into(),
            content_generation: "content-1".into(),
            relative_path: "album/photo.jpg".into(),
            is_favorite: false,
            asset_date_ms: None,
            is_hidden: false,
            original: asset("original"),
            thumbnail: asset("thumbnail"),
            thumbnail_source_sha256: digest.hex,
            events: Default::default(),
        }],
        ..Default::default()
    }
}
fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Runtime::new().unwrap()
}
fn revisions(f: &Fixture) {
    let asset = json!({"recordName":"cloud-photo","recordType":"CPLAsset","recordChangeTag":"revision","fields":{}});
    let master = json!({"recordName":"cloud-master","recordType":"CPLMaster","recordChangeTag":"revision","fields":{}});
    f.service.pool().get().unwrap().execute(
        "UPDATE sync_entries SET native_pending_asset=?2,native_pending_master=?3 WHERE job_id=?1 AND relative_path='album/photo.jpg'",
        rusqlite::params![f.job.id,asset.to_string(),master.to_string()],
    ).unwrap();
}

#[test]
fn ordinary_local_indexing_does_not_store_sha256_or_enroll_a_sync_entry() {
    let f = fixture();
    let path = f.root.join("album/photo.jpg");
    std::fs::write(&path, jpeg([20, 40, 60])).unwrap();
    f.service
        .store()
        .commit_local_scan(vec![LocalBackend::new_item_from_path(&path)
            .unwrap()
            .unwrap()])
        .unwrap();
    let count: i64 = f
        .service
        .pool()
        .get()
        .unwrap()
        .query_row("SELECT COUNT(*) FROM sync_entries", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 0);
    let sha_columns: i64 = f
        .service
        .pool()
        .get()
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('media_items') WHERE name LIKE '%sha256%'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(sha_columns, 0);
}

#[test]
fn same_album_name_size_and_digest_merge_even_outside_upload_scope() {
    let f = fixture();
    let bytes = jpeg([20, 40, 60]);
    std::fs::write(f.root.join("album/photo.jpg"), &bytes).unwrap();
    let graph = graph(&bytes);
    rt().block_on(async {
        f.service
            .prepare_local_matches(f.job.id, &graph)
            .await
            .unwrap();
        let measured = f.service.measure_local_matches(f.job.id, &graph).await;
        assert_eq!(measured.len(), 1);
        f.service
            .preserve_collisions(f.job.id, &graph, &measured)
            .await
            .unwrap();
        revisions(&f);
        let (_, _, owed, applied) = f
            .service
            .store()
            .promote_remote_graph(f.job.id, graph, "target", measured)
            .unwrap();
        assert_eq!(owed, 0);
        assert!(applied);
    });
    assert_eq!(
        std::fs::read(f.root.join("album/photo.jpg")).unwrap(),
        bytes
    );
    assert!(!f.root.join("album/photo_1.jpg").exists());
    assert_eq!(
        crate::core::db::list_all_media(f.service.pool())
            .unwrap()
            .len(),
        1
    );
    let entries = f.service.store().managed_photos(f.job.id).unwrap();
    assert_eq!(entries.len(), 1);
    assert!(entries[0].remote);
}

#[test]
fn equal_size_different_bytes_preserve_the_local_file_with_a_free_numeric_suffix() {
    let f = fixture();
    let mut local = jpeg([20, 40, 60]);
    let mut remote = jpeg([60, 20, 40]);
    let size = local.len().max(remote.len());
    local.resize(size, 0);
    remote.resize(size, 0);
    std::fs::write(f.root.join("album/photo.jpg"), &local).unwrap();
    std::fs::write(f.root.join("album/photo_1.jpg"), b"existing one").unwrap();
    std::fs::write(f.root.join("album/photo_2.jpg"), b"existing two").unwrap();
    let mut graph = graph(&remote);
    let mut reserved = graph.photos[0].clone();
    reserved.photo_id = "other-photo".into();
    reserved.relative_path = "album/photo_3.jpg".into();
    graph.photos.push(reserved);
    rt().block_on(async {
        f.service
            .prepare_local_matches(f.job.id, &graph)
            .await
            .unwrap();
        let measured = f.service.measure_local_matches(f.job.id, &graph).await;
        assert_eq!(measured.len(), 1);
        f.service
            .preserve_collisions(f.job.id, &graph, &measured)
            .await
            .unwrap();
    });
    assert!(!f.root.join("album/photo.jpg").exists());
    assert_eq!(
        std::fs::read(f.root.join("album/photo_4.jpg")).unwrap(),
        local
    );
    assert_eq!(
        std::fs::read(f.root.join("album/photo_1.jpg")).unwrap(),
        b"existing one"
    );
    assert_eq!(
        std::fs::read(f.root.join("album/photo_2.jpg")).unwrap(),
        b"existing two"
    );
    let media = crate::core::db::list_all_media(f.service.pool()).unwrap();
    assert_eq!(media[0].path, f.root.join("album/photo_4.jpg"));
    let entries = f.service.store().unclaimed_local_entries(f.job.id).unwrap();
    assert_eq!(entries[0].relative_path, "album/photo_4.jpg");
    assert!(entries[0].cached.is_none());
}

#[test]
fn different_sizes_do_not_hash_before_preserving_the_local_file() {
    let f = fixture();
    let local = jpeg([20, 40, 60]);
    std::fs::write(f.root.join("album/photo.jpg"), &local).unwrap();
    let mut remote = local.clone();
    remote.extend_from_slice(b"longer");
    let graph = graph(&remote);
    rt().block_on(async {
        f.service
            .prepare_local_matches(f.job.id, &graph)
            .await
            .unwrap();
        let measured = f.service.measure_local_matches(f.job.id, &graph).await;
        assert!(measured.is_empty());
        f.service
            .preserve_collisions(f.job.id, &graph, &measured)
            .await
            .unwrap();
    });
    assert_eq!(
        std::fs::read(f.root.join("album/photo_1.jpg")).unwrap(),
        local
    );
    let digests: i64 = f
        .service
        .pool()
        .get()
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM sync_entries WHERE native_local_digest IS NOT NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(digests, 0);
}

#[test]
fn a_matching_record_id_skips_path_collision_hashing_and_renaming() {
    let f = fixture();
    let bytes = jpeg([20, 40, 60]);
    std::fs::write(f.root.join("album/photo.jpg"), &bytes).unwrap();
    let graph = graph(&bytes);
    rt().block_on(async {
        f.service.prepare_local_matches(f.job.id,&graph).await.unwrap();
        f.service.pool().get().unwrap().execute("UPDATE sync_entries SET native_photo_id='cloud-photo',native_master_id='cloud-master',native_dirty=4 WHERE job_id=?1",[f.job.id]).unwrap();
        let measured=f.service.measure_local_matches(f.job.id,&graph).await;assert!(measured.is_empty());
        f.service.preserve_collisions(f.job.id,&graph,&measured).await.unwrap();
    });
    assert!(f.root.join("album/photo.jpg").exists());
    assert!(!f.root.join("album/photo_1.jpg").exists());
}

#[test]
fn failed_collision_database_commit_restores_the_original_name_and_bytes() {
    let f = fixture();
    let local = jpeg([20, 40, 60]);
    std::fs::write(f.root.join("album/photo.jpg"), &local).unwrap();
    let mut remote = local.clone();
    remote.push(0);
    let graph = graph(&remote);
    rt().block_on(async {
        f.service.prepare_local_matches(f.job.id,&graph).await.unwrap();
        let measured=f.service.measure_local_matches(f.job.id,&graph).await;
        f.service.pool().get().unwrap().execute_batch("CREATE TRIGGER reject_collision BEFORE UPDATE OF relative_path ON sync_entries BEGIN SELECT RAISE(ABORT,'injected relocation failure'); END;").unwrap();
        assert!(f.service.preserve_collisions(f.job.id,&graph,&measured).await.is_err());
    });
    assert_eq!(
        std::fs::read(f.root.join("album/photo.jpg")).unwrap(),
        local
    );
    assert!(!f.root.join("album/photo_1.jpg").exists());
    assert_eq!(
        crate::core::db::list_all_media(f.service.pool()).unwrap()[0].path,
        f.root.join("album/photo.jpg")
    );
}

#[test]
fn interrupted_collision_move_is_restored_by_publication_recovery() {
    let f = fixture();
    let bytes = jpeg([20, 40, 60]);
    let source = f.root.join("album/photo.jpg");
    let preserved = f.root.join("album/photo_1.jpg");
    std::fs::write(&source, &bytes).unwrap();
    let graph = graph(&bytes);
    rt().block_on(f.service.prepare_local_matches(f.job.id, &graph))
        .unwrap();
    let entry = f
        .service
        .store()
        .unclaimed_local_entries(f.job.id)
        .unwrap()
        .remove(0);
    let details = json!({"collision":true,"source":source,"preserved":preserved,"root":f.root,"stamp":stamp(&source).unwrap()});
    let capture = json!({"job":f.job.id,"entry":entry.entry_id,"local":entry.local_version,"dirty":1,"config":f.job.config_generation,"photo":"cloud-photo"});
    f.service
        .store()
        .runtime(Command::BeginCollision {
            id: "collision-interrupted".into(),
            capture,
            details,
        })
        .unwrap();
    std::fs::hard_link(&source, &preserved).unwrap();
    std::fs::remove_file(&source).unwrap();
    f.service.recover_publications(f.job.id).unwrap();
    assert_eq!(std::fs::read(source).unwrap(), bytes);
    assert!(!preserved.exists());
    assert_eq!(
        f.service.store().unclaimed_local_entries(f.job.id).unwrap()[0].relative_path,
        "album/photo.jpg"
    );
}
