//! File/actor publication regressions. Transport coverage uses the actual server
//! in native_sync_assets; these cases never set a resource directly to Verified.
use super::*;
use crate::core::sync::{
    native::{records::RecordSet, NativeRecord},
    store::{
        tests::{new_job, temp_root},
        InboxEvent, InboxKind,
    },
    AssetDescriptor, RemotePhoto, SyncJob,
};

/// Promotion with nothing measured. These cases exercise bookkeeping; a measured
/// digest is what would let an entry adopt a cloud identity instead.
fn no_local_digests() -> std::collections::HashMap<String, crate::core::sync::content::ContentDigest>
{
    std::collections::HashMap::new()
}

const GENERATION: &str = "501855bb-18d3-4008-abc3-37e4948bf6f0";
struct Fixture {
    service: SyncService,
    job: SyncJob,
    root: PathBuf,
}
fn fixture(name: &str) -> Fixture {
    crate::testing::Sandbox::install();
    let directory = temp_root(name);
    let root = temp_root(&format!("{name}-library"));
    let pool = crate::core::db::init_pool(&directory.join("library.sqlite")).unwrap();
    let service = SyncService::with_staging_root_for_tests(pool, directory.join("staging"));
    let job = service.store().create_job(&new_job(root.clone())).unwrap();
    service
        .store()
        .begin_native_receive(
            job.id,
            crate::core::sync::native::demo_pinned_scope(
                "https://sync.example.invalid/",
                &["PrimarySync"],
            )
            .namespace,
            GENERATION,
            "scope-1",
            crate::core::sync::NativeZone::new("PrimarySync"),
        )
        .unwrap();
    Fixture { service, job, root }
}
fn jpeg(color: u32) -> Vec<u8> {
    let image = gdk_pixbuf::Pixbuf::new(gdk_pixbuf::Colorspace::Rgb, false, 8, 64, 48).unwrap();
    image.fill(color);
    image.save_to_bufferv("jpeg", &[]).unwrap()
}
fn descriptor(id: &str, bytes: &[u8]) -> AssetDescriptor {
    AssetDescriptor {
        asset_id: id.into(),
        size: bytes.len() as u64,
        file_checksum: ContentDigest::of(bytes).base64,
        mime_type: "image/jpeg".into(),
        width: Some(64),
        height: Some(48),
    }
}
fn project(f: &Fixture, revision: &str, original: &[u8], thumbnail: &[u8]) -> RemotePhoto {
    let content = uuid::Uuid::new_v4().to_string();
    let original = descriptor(&format!("original-{revision}"), original);
    let thumbnail = descriptor(&format!("thumbnail-{revision}"), thumbnail);
    let field = |kind: &str, value: Value| json!({"type":kind,"value":value});
    let records = [
        json!({"recordName":"master","recordType":"CPLMaster","recordChangeTag":format!("master-{revision}"),"fields":{"photoId":field("STRING",json!("photo")),"contentGeneration":field("STRING",json!(content)),"thumbnailSourceOriginalSHA256":field("STRING",json!(ContentDigest::hex_from_descriptor_base64(&original.file_checksum).unwrap())),"resOriginalRes":field("ASSETID",serde_json::to_value(original).unwrap()),"resJPEGThumbRes":field("ASSETID",serde_json::to_value(thumbnail).unwrap())}}),
        json!({"recordName":"photo","recordType":"CPLAsset","recordChangeTag":format!("asset-{revision}"),"fields":{"masterRef":field("REFERENCE",json!({"recordName":"master","action":"NONE"})),"contentGeneration":field("STRING",json!(content)),"relativePath":field("STRING",json!("camera/photo.jpg")),"isFavorite":field("INT64",json!(1))}}),
    ];
    let events: Vec<_> = records
        .into_iter()
        .map(|record| {
            let typed: NativeRecord = serde_json::from_value(record.clone()).unwrap();
            InboxEvent {
                ordinal: 0,
                record_name: typed.record_name,
                record_type: typed.record_type.unwrap(),
                record_change_tag: typed.record_change_tag.unwrap(),
                kind: InboxKind::Updated,
                payload: record,
            }
        })
        .collect();
    f.service
        .store()
        .save_delta_page(f.job.id, events, &format!("token-{revision}"))
        .unwrap();
    let events = f
        .service
        .store()
        .inbox_events(f.job.id, GENERATION)
        .unwrap();
    let graph = RecordSet::from_events(&events)
        .unwrap()
        .to_graph(&crate::core::sync::native::receive::ZoneId::from(
            &crate::core::sync::NativeZone::new("PrimarySync"),
        ))
        .unwrap();
    let photo = graph.photos[0].clone();
    f.service
        .store()
        .promote_remote_graph(
            f.job.id,
            graph,
            &format!("token-{revision}"),
            no_local_digests(),
        )
        .unwrap();
    photo
}
fn stage(f: &Fixture, photo: &RemotePhoto, original: &[u8], thumbnail: &[u8]) {
    for (asset, role, bytes) in [
        (&photo.original.asset_id, AssetRole::Original, original),
        (&photo.thumbnail.asset_id, AssetRole::Thumbnail, thumbnail),
    ] {
        let path = f.service.stage_verified(asset, role, bytes).unwrap();
        f.service
            .store()
            .stage_resource(f.job.id, asset, role.as_str(), &path.to_string_lossy())
            .unwrap();
    }
}
fn admit(f: &Fixture) -> TransferReport {
    let mut report = TransferReport::default();
    f.service
        .admit_complete_publications(f.job.id, &mut report)
        .unwrap();
    report
}
#[test]
fn verified_bytes_are_staged_under_an_owned_name_not_a_url() {
    let f = fixture("native-stage-owned");
    let bytes = jpeg(0x7799AAFF);
    let path = f
        .service
        .stage_verified("asset-1", AssetRole::Original, &bytes)
        .unwrap();
    assert!(path.starts_with(f.service.staging_root()));
    assert_eq!(std::fs::read(path).unwrap(), bytes);
}
#[test]
fn a_hostile_asset_id_cannot_escape_the_staging_directory() {
    let f = fixture("native-stage-hostile");
    let path = f
        .service
        .stage_verified("../../etc/passwd", AssetRole::Original, b"owned")
        .unwrap();
    assert!(path.starts_with(f.service.staging_root()));
    assert!(!path.to_string_lossy().contains(".."));
}
#[test]
fn a_photo_with_only_its_original_staged_is_never_admitted() {
    let f = fixture("native-half-pair");
    let original = jpeg(0x223344FF);
    let thumbnail = jpeg(0x223344FF);
    let photo = project(&f, "one", &original, &thumbnail);
    let path = f
        .service
        .stage_verified(&photo.original.asset_id, AssetRole::Original, &original)
        .unwrap();
    f.service
        .store()
        .stage_resource(
            f.job.id,
            &photo.original.asset_id,
            "original",
            &path.to_string_lossy(),
        )
        .unwrap();
    assert_eq!(admit(&f).admitted, 0);
    assert!(crate::core::db::list_all_media(f.service.pool())
        .unwrap()
        .is_empty());
    assert!(!f.root.join("camera/photo.jpg").exists());
}
#[test]
fn a_complete_staged_pair_is_published_and_admitted_exactly_once() {
    let f = fixture("native-complete-pair");
    let original = jpeg(0x446688FF);
    let thumbnail = jpeg(0x446688FF);
    let photo = project(&f, "one", &original, &thumbnail);
    stage(&f, &photo, &original, &thumbnail);
    let report = admit(&f);
    assert_eq!(report.admitted, 1, "{report:?}");
    assert!(report.failures.is_empty(), "{report:?}");
    assert_eq!(admit(&f).admitted, 0);
    assert_eq!(
        std::fs::read(f.root.join("camera/photo.jpg")).unwrap(),
        original
    );
    let media = crate::core::db::list_all_media(f.service.pool()).unwrap();
    assert_eq!(media.len(), 1);
    assert!(media[0].is_favorite);
    assert_eq!(
        f.service
            .store()
            .unresolved_resource_count(f.job.id)
            .unwrap(),
        0
    );
}
#[test]
fn a_checksum_failure_preserves_the_previous_complete_photo() {
    let f = fixture("native-pair-mismatch");
    let old = jpeg(0x337799FF);
    let first = project(&f, "one", &old, &old);
    stage(&f, &first, &old, &old);
    assert_eq!(admit(&f).admitted, 1);
    let new = jpeg(0xCC2244FF);
    let second = project(&f, "two", &new, &new);
    stage(&f, &second, &new, &new);
    f.service
        .stage_verified(
            &second.thumbnail.asset_id,
            AssetRole::Thumbnail,
            b"not the promised thumbnail",
        )
        .unwrap();
    let report = admit(&f);
    assert_eq!(report.admitted, 0);
    assert_eq!(report.failures.len(), 1);
    assert_eq!(std::fs::read(f.root.join("camera/photo.jpg")).unwrap(), old);
    assert_eq!(
        crate::core::db::list_all_media(f.service.pool())
            .unwrap()
            .len(),
        1
    );
    assert!(!f
        .service
        .store()
        .pending_publications(f.job.id)
        .unwrap()
        .is_empty());
}
#[test]
fn a_database_failure_after_replace_restores_the_old_file_and_can_retry() {
    let f = fixture("native-pair-rollback");
    let old = jpeg(0x337799FF);
    let first = project(&f, "one", &old, &old);
    stage(&f, &first, &old, &old);
    assert_eq!(admit(&f).admitted, 1);
    let id = crate::core::db::list_all_media(f.service.pool()).unwrap()[0].id;
    let new = jpeg(0xCC2244FF);
    let second = project(&f, "two", &new, &new);
    stage(&f, &second, &new, &new);
    f.service.pool().get().unwrap().execute_batch("CREATE TRIGGER refuse_publication BEFORE UPDATE OF state ON sync_publications WHEN NEW.state='complete' BEGIN SELECT RAISE(ABORT,'injected admission failure'); END;").unwrap();
    let report = admit(&f);
    assert_eq!(report.admitted, 0);
    assert_eq!(report.failures.len(), 1);
    assert_eq!(std::fs::read(f.root.join("camera/photo.jpg")).unwrap(), old);
    assert_eq!(
        crate::core::db::list_all_media(f.service.pool()).unwrap()[0].id,
        id
    );
    f.service
        .pool()
        .get()
        .unwrap()
        .execute_batch("DROP TRIGGER refuse_publication")
        .unwrap();
    f.service.recover_publications(f.job.id).unwrap();
    let report = admit(&f);
    assert_eq!(report.admitted, 1, "{report:?}");
    assert!(report.failures.is_empty(), "{report:?}");
    assert_eq!(std::fs::read(f.root.join("camera/photo.jpg")).unwrap(), new);
    let media = crate::core::db::list_all_media(f.service.pool()).unwrap();
    assert_eq!(media.len(), 1);
    assert_eq!(media[0].id, id);
}
