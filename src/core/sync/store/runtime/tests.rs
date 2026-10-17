//! Callback guards use the actual actor and SQLite transaction; no dirty bit
//! is cleared in fixture setup or by a simulated success outside the command.
use super::*;
use crate::core::sync::store::tests::new_job;
use crate::core::{
    db_actor::{start_db_actor, DbActorHandle, DbCommand},
    events::DomainEventSender,
    identity::MediaId,
    sync::native::{LeaseGrant, NativeZone},
};
struct Fixture {
    store: SyncStore,
    actor: DbActorHandle,
    epoch: Epoch,
    operation: Issued,
    media: i64,
    _root: tempfile::TempDir,
}
fn fixture(name: &str) -> Fixture {
    let root = crate::testing::Sandbox::install().scratch(name);
    let library = root.path().join("library");
    std::fs::create_dir_all(&library).unwrap();
    let pool = crate::core::db::init_pool(&root.path().join("library.sqlite")).unwrap();
    let (sender, _events) = DomainEventSender::new();
    let actor = start_db_actor(pool.clone(), sender);
    let store = SyncStore::with_actor(pool.clone(), actor.clone());
    let job = store.create_job(&new_job(library.clone())).unwrap();
    let generation = "501855bb-18d3-4008-abc3-37e4948bf6f0";
    store
        .begin_native_receive(
            job.id,
            crate::core::sync::native::demo_pinned_scope(
                "https://sync.example.invalid/",
                &["PrimarySync"],
            )
            .namespace,
            generation,
            "scope",
            NativeZone::new("PrimarySync"),
        )
        .unwrap();
    let path = library.join("photo.jpg");
    let image = gdk_pixbuf::Pixbuf::new(gdk_pixbuf::Colorspace::Rgb, false, 8, 4, 4).unwrap();
    image.fill(0x77AA33FF);
    std::fs::write(&path, image.save_to_bufferv("jpeg", &[]).unwrap()).unwrap();
    let item = crate::core::media::NewMediaItem {
        uri: crate::core::file_uri::from_path(&path),
        path: path.clone(),
        folder_path: library,
        mime_type: "image/jpeg".into(),
        media_subkind: "standard".into(),
        media_attributes: "{}".into(),
        width: Some(4),
        height: Some(4),
        video_duration_secs: None,
        taken_at: None,
        file_mtime: std::fs::metadata(&path).unwrap().modified().unwrap().into(),
        file_size: std::fs::metadata(&path).unwrap().len(),
        blake3_hash: String::new(),
    };
    let media = crate::core::db::upsert_media_items_batch(&pool, &[item]).unwrap()[0].id;
    // A previous session left one real metadata obligation and known cloud IDs.
    pool.get().unwrap().execute("INSERT INTO sync_entries(job_id,media_id,relative_path,native_dirty,native_local_version,native_metadata_version,native_photo_id,native_master_id,native_asset_revision,native_master_revision,native_content_generation) VALUES(?1,?2,'photo.jpg',2,1,1,'photo','master','old-asset','old-master','11111111-1111-4111-8111-111111111111')",params![job.id,media]).unwrap();
    pool.get()
        .unwrap()
        .execute("UPDATE media_items SET is_favorite=1 WHERE id=?1", [media])
        .unwrap();
    let entry = store.uplink_view(job.id).unwrap()[0].entry_id;
    let epoch = Epoch {
        job: job.id,
        config: job.config_generation,
        cycle: uuid::Uuid::new_v4().to_string(),
    };
    store.runtime(Command::Begin(epoch.clone())).unwrap();
    let operation = Issued {
        id: format!("pv-{}", uuid::Uuid::new_v4()),
        job: job.id,
        config: job.config_generation,
        entry,
        generation: generation.into(),
        phase: "modify".into(),
        versions: [1, 1, 0, 0],
        dirty: 2,
        photo: "photo".into(),
        master: "master".into(),
        content_generation: "11111111-1111-4111-8111-111111111111".into(),
        body: json!({"operations":[{"operationType":"update","record":{"recordName":"photo","recordChangeTag":"old-asset","fields":{"isFavorite":{"type":"INT64","value":1}}}}]}),
        preparation: Value::Null,
    };
    store
        .runtime(Command::Save(epoch.clone(), operation.clone()))
        .unwrap();
    Fixture {
        store,
        actor,
        epoch,
        operation,
        media,
        _root: root,
    }
}
fn lease(ttl: u64) -> UploadLease {
    UploadLease::from_grant(
        &LeaseGrant {
            lease_id: uuid::Uuid::new_v4().to_string(),
            lease_token: "isolated-fixture-token".into(),
            fencing_token: "1".into(),
            ttl_seconds: ttl,
            heartbeat_seconds: 1,
            phase: "modify".into(),
        },
        std::time::Instant::now(),
    )
}
fn response() -> Value {
    json!({"records":[{"recordName":"photo","recordType":"CPLAsset","recordChangeTag":"confirmed-asset","fields":{"isFavorite":{"type":"INT64","value":1},"relativePath":{"type":"STRING","value":"photo.jpg"},"masterRef":{"type":"REFERENCE","value":{"recordName":"master","action":"NONE"}},"contentGeneration":{"type":"STRING","value":"11111111-1111-4111-8111-111111111111"}}}]})
}
#[test]
fn a_new_local_edit_is_not_cleared_by_an_older_success() {
    let f = fixture("callback-new-edit");
    f.actor
        .execute_blocking(DbCommand::SetFavorite {
            ids: vec![MediaId::from(f.media)],
            is_favorite: false,
        })
        .unwrap();
    f.store
        .runtime(Command::Confirm {
            epoch: f.epoch.clone(),
            operation: f.operation.clone(),
            response: response(),
            lease: lease(90),
        })
        .unwrap();
    let entry = f.store.uplink_view(f.epoch.job).unwrap().pop().unwrap();
    assert!(entry.dirty.contains(2));
    assert_eq!(entry.metadata_version, 2);
    assert_eq!(entry.asset_revision.as_deref(), Some("confirmed-asset"));
}
#[test]
fn an_expired_lease_never_clears_dirty() {
    let f = fixture("callback-expired");
    let result = f.store.runtime(Command::Confirm {
        epoch: f.epoch.clone(),
        operation: f.operation.clone(),
        response: response(),
        lease: lease(0),
    });
    assert!(result.is_err());
    assert!(f.store.uplink_view(f.epoch.job).unwrap()[0]
        .dirty
        .contains(2));
    assert_eq!(f.store.pending_issued(f.epoch.job).unwrap().len(), 1);
}
#[test]
fn a_new_cycle_rejects_the_previous_cycles_callback() {
    let f = fixture("callback-old-cycle");
    let mut next = f.epoch.clone();
    next.cycle = uuid::Uuid::new_v4().to_string();
    f.store.runtime(Command::Begin(next)).unwrap();
    assert!(f
        .store
        .runtime(Command::Confirm {
            epoch: f.epoch.clone(),
            operation: f.operation.clone(),
            response: response(),
            lease: lease(90)
        })
        .is_err());
    assert!(f.store.uplink_view(f.epoch.job).unwrap()[0]
        .dirty
        .contains(2));
}
#[test]
fn an_inline_error_in_any_atomic_result_refuses_confirmation() {
    let mut f = fixture("callback-atomic-error");
    f.operation.body["operations"].as_array_mut().unwrap().push(json!({"operationType":"update","record":{"recordName":"master","recordChangeTag":"old-master","fields":{}}}));
    f.store
        .runtime(Command::Save(f.epoch.clone(), f.operation.clone()))
        .unwrap();
    let mut result = response();
    result["records"]
        .as_array_mut()
        .unwrap()
        .push(json!({"recordName":"master","serverErrorCode":"CONFLICT"}));
    assert!(f
        .store
        .runtime(Command::Confirm {
            epoch: f.epoch.clone(),
            operation: f.operation.clone(),
            response: result,
            lease: lease(90)
        })
        .is_err());
    assert!(f.store.uplink_view(f.epoch.job).unwrap()[0]
        .dirty
        .contains(2));
}

#[test]
fn metadata_edits_preserve_the_received_capture_dates_subseconds() {
    let f = fixture("metadata-subsecond-date");
    let item = crate::core::db::get_media_item(&f.store.pool, f.media).unwrap();
    let bytes = std::fs::read(&item.path).unwrap();
    let digest = crate::core::sync::ContentDigest::of(&bytes);
    let asset = crate::core::sync::AssetDescriptor {
        asset_id: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa".into(),
        size: digest.size,
        file_checksum: digest.base64,
        mime_type: "image/jpeg".into(),
        width: Some(4),
        height: Some(4),
    };
    let photo = crate::core::sync::RemotePhoto {
        photo_id: "photo".into(),
        master_id: "master".into(),
        content_generation: "11111111-1111-4111-8111-111111111111".into(),
        relative_path: "photo.jpg".into(),
        is_favorite: true,
        asset_date_ms: Some(1_700_000_000_123),
        is_hidden: false,
        original: asset.clone(),
        thumbnail: asset,
        thumbnail_source_sha256: digest.hex,
        events: Default::default(),
    };
    let conn = f.store.pool.get().unwrap();
    conn.execute(
        "UPDATE media_items SET taken_at=1700000000 WHERE id=?1",
        [f.media],
    )
    .unwrap();
    conn.execute(
        "UPDATE sync_entries SET native_resource_baseline=?2 WHERE id=?1",
        params![f.operation.entry, serde_json::to_string(&photo).unwrap()],
    )
    .unwrap();
    drop(conn);
    let metadata = f.store.local_metadata(f.operation.entry).unwrap();
    assert_eq!(metadata["assetDate"]["value"], 1_700_000_000_123i64);
}

#[test]
fn uncertain_mutations_cannot_be_discarded_as_superseded_preparation() {
    let f = fixture("uncertain-retirement");
    assert!(f
        .store
        .runtime(Command::RetireIssued {
            epoch: f.epoch.clone(),
            operation: f.operation.clone(),
            rejected: false,
            reason: String::new(),
        })
        .is_err());
    assert_eq!(f.store.pending_issued(f.epoch.job).unwrap().len(), 1);
    assert_eq!(f.store.uplink_view(f.epoch.job).unwrap()[0].dirty.bits(), 2);
}
#[test]
fn a_definitive_atomic_rejection_keeps_dirty_and_unblocks_a_new_decision() {
    let f = fixture("rejected-retirement");
    f.store
        .runtime(Command::RetireIssued {
            epoch: f.epoch.clone(),
            operation: f.operation.clone(),
            rejected: true,
            reason: "HTTP 400 BAD_REQUEST: Invalid record request".into(),
        })
        .unwrap();
    assert!(f.store.pending_issued(f.epoch.job).unwrap().is_empty());
    assert_eq!(f.store.uplink_view(f.epoch.job).unwrap()[0].dirty.bits(), 2);
}

#[test]
fn authentication_upgrade_keeps_scope_but_cannot_replace_an_accepted_account() {
    use crate::core::sync::native::{NativeAuthentication, NativeUser};
    let f = fixture("authentication-identity");
    let job = f.store.get_job(f.epoch.job).unwrap().unwrap();
    let original = f.store.native_scope(job.connection_id).unwrap().unwrap();
    let mut accepted = original.clone();
    accepted.authentication = Some(NativeAuthentication {
        mode: "username_password".into(),
        user: Some(NativeUser {
            id: "11111111-1111-4111-8111-111111111111".into(),
            username: "owner".into(),
        }),
        resource_bearer_required: true,
    });
    f.store
        .runtime(Command::AcceptAuthentication {
            connection: job.connection_id,
            expected: Box::new(original),
            updated: Box::new(accepted.clone()),
        })
        .unwrap();
    let mut foreign = accepted.clone();
    foreign
        .authentication
        .as_mut()
        .unwrap()
        .user
        .as_mut()
        .unwrap()
        .id = "22222222-2222-4222-8222-222222222222".into();
    assert!(f
        .store
        .runtime(Command::AcceptAuthentication {
            connection: job.connection_id,
            expected: Box::new(accepted.clone()),
            updated: Box::new(foreign)
        })
        .is_err());
    assert_eq!(
        f.store.native_scope(job.connection_id).unwrap().unwrap(),
        accepted
    );
}

#[test]
fn upload_failure_is_persisted_for_one_entry_until_the_next_accepted_cycle() {
    let f = fixture("upload-failure-cycle");
    let entry = f.operation.entry;
    let before = f.store.uplink_view(f.epoch.job).unwrap();
    assert!(f.store.upload_error(f.epoch.job, entry).unwrap().is_none());
    let other_job_entry = {
        let conn = f.store.pool.get().unwrap();
        conn.execute(
            "INSERT INTO sync_jobs(connection_id,local_root,native_zone_name) SELECT connection_id,local_root||'-other','OtherZone' FROM sync_jobs WHERE id=?1",
            [f.epoch.job],
        ).unwrap();
        let job = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO sync_entries(job_id,relative_path,native_dirty,native_upload_error) VALUES(?1,'other-job.jpg',1,'other job failure')",
            [job],
        ).unwrap();
        (job, conn.last_insert_rowid())
    };
    assert!(f
        .store
        .runtime(Command::UploadFailed {
            epoch: f.epoch.clone(),
            entry: other_job_entry.1,
            reason: "wrong job".into(),
        })
        .is_err());
    let other = {
        let conn = f.store.pool.get().unwrap();
        conn.execute(
            "INSERT INTO sync_entries(job_id,relative_path,native_dirty) VALUES(?1,'other.jpg',1)",
            [f.epoch.job],
        )
        .unwrap();
        conn.last_insert_rowid()
    };
    f.store
        .runtime(Command::UploadFailed {
            epoch: f.epoch.clone(),
            entry,
            reason: "original upload could not be completed".into(),
        })
        .unwrap();
    assert_eq!(
        f.store.upload_error(f.epoch.job, entry).unwrap().as_deref(),
        Some("original upload could not be completed")
    );
    assert!(f.store.upload_error(f.epoch.job, other).unwrap().is_none());
    assert_eq!(f.store.uplink_view(f.epoch.job).unwrap()[0], before[0]);
    assert_eq!(f.store.pending_issued(f.epoch.job).unwrap().len(), 1);
    f.store.runtime(Command::End(f.epoch.clone())).unwrap();
    assert!(f.store.upload_error(f.epoch.job, entry).unwrap().is_some());

    let mut invalid = f.epoch.clone();
    invalid.config += 1;
    assert!(f.store.runtime(Command::Begin(invalid)).is_err());
    assert!(f.store.upload_error(f.epoch.job, entry).unwrap().is_some());

    let mut next = f.epoch.clone();
    next.cycle = uuid::Uuid::new_v4().to_string();
    f.store.runtime(Command::Begin(next)).unwrap();
    assert!(f.store.upload_error(f.epoch.job, entry).unwrap().is_none());
    assert_eq!(
        f.store
            .upload_error(other_job_entry.0, other_job_entry.1)
            .unwrap()
            .as_deref(),
        Some("other job failure")
    );
    assert!(f
        .store
        .runtime(Command::UploadFailed {
            epoch: f.epoch,
            entry,
            reason: "late result from the previous cycle".into(),
        })
        .is_err());
    assert!(f
        .store
        .upload_error(f.operation.job, entry)
        .unwrap()
        .is_none());
    assert_eq!(f.store.pending_issued(f.operation.job).unwrap().len(), 1);
}
