//! Recovery fixtures represent operations saved before an application restart.
//! Failures then go through the real client, SQLite actor and SyncServer.
use super::*;
use photo_viewer::core::{
    db::{self, DbPool},
    db_actor::{start_db_actor, DbActorHandle, DbCommand},
    events::{ChangeSource, DomainEventSender},
    identity::MediaId,
    media::NewMediaItem,
    sync::{
        store::DirtyEntry, NewSyncJob, ProviderKind, SyncDirection, SyncJob, SyncService,
        UploadScope,
    },
};

struct Fixture {
    _server: Server,
    root: tempfile::TempDir,
    source: PathBuf,
    pool: DbPool,
    actor: DbActorHandle,
    service: SyncService,
    job: SyncJob,
}
impl Fixture {
    async fn new(server: Server) -> Self {
        let sandbox = photo_viewer::testing::Sandbox::install();
        let root = sandbox.scratch("upload-failure-recovery");
        let source = root.path().join("source");
        std::fs::create_dir_all(&source).unwrap();
        let pool = db::init_pool(&root.path().join("library.sqlite")).unwrap();
        let (sender, _events) = DomainEventSender::new();
        let actor = start_db_actor(pool.clone(), sender);
        let key = server.api_key.clone();
        let service = SyncService::with_actor(pool.clone(), actor.clone())
            .with_credential_reader(move |_| Ok(key.clone()));
        let scope = server.client().probe().await.unwrap();
        let job = service
            .store()
            .create_job(&NewSyncJob {
                provider_kind: ProviderKind::NativeSyncServer,
                endpoint: server.base_url.clone(),
                credential_ref: "upload-failure-key".into(),
                local_root: source.clone(),
                direction: SyncDirection::Bidirectional,
                upload_scope: UploadScope::All,
                upload_albums: vec![],
                native_zone: Some(NativeZone::new("PrimarySync")),
                native_pinned: Some(scope),
            })
            .unwrap();
        let f = Self {
            _server: server,
            root,
            source,
            pool,
            actor,
            service,
            job,
        };
        f.add(0..3);
        let report = f.service.run_cycle(f.job.id).await.unwrap();
        assert!(report.failures.is_empty(), "{report:?}");
        assert_eq!(report.mutations_confirmed, 3, "{report:?}");
        f
    }
    fn item(path: &Path) -> NewMediaItem {
        NewMediaItem {
            uri: photo_viewer::core::file_uri::from_path(path),
            path: path.into(),
            folder_path: path.parent().unwrap().into(),
            mime_type: "image/jpeg".into(),
            media_subkind: "standard".into(),
            media_attributes: "{}".into(),
            width: None,
            height: None,
            video_duration_secs: None,
            taken_at: None,
            file_mtime: std::fs::metadata(path).unwrap().modified().unwrap().into(),
            file_size: std::fs::metadata(path).unwrap().len(),
            blake3_hash: String::new(),
        }
    }
    fn add(&self, indices: std::ops::Range<usize>) {
        let items = indices
            .map(|i| {
                Self::item(&common::write_plain_jpeg(
                    &self.source,
                    &format!("photo-{i}.jpg"),
                ))
            })
            .collect();
        self.actor
            .execute_blocking(DbCommand::UpsertMediaBatch {
                source: ChangeSource::UserInteractive,
                items,
            })
            .unwrap();
    }
    fn failure(&self, entry: i64) -> Option<String> {
        self.pool
            .get()
            .unwrap()
            .query_row(
                "SELECT native_upload_error FROM sync_entries WHERE id=?1",
                [entry],
                |row| row.get(0),
            )
            .unwrap()
    }
    fn capture(&self, entry: &DirtyEntry) -> serde_json::Value {
        let scope = self
            .service
            .store()
            .native_scope(self.job.connection_id)
            .unwrap()
            .unwrap();
        json!({
            "id": format!("pv-{}", uuid::Uuid::new_v4()), "job": self.job.id,
            "config": self.job.config_generation, "entry": entry.entry_id,
            "generation": scope.database_generation, "phase": "modify",
            "versions": [entry.local_version, entry.metadata_version, entry.content_version, entry.deletion_version],
            "dirty": entry.dirty.bits(), "photo": entry.photo_id, "master": entry.master_id,
            "content_generation": uuid::Uuid::new_v4().to_string(), "body": null,
            "preparation": {
                "metadata": {"isFavorite":{"type":"INT64","value":1}, "relativePath":{"type":"STRING","value":entry.relative_path}},
                "events": {"metadataEvent":entry.metadata_event,"contentEvent":entry.content_event,"deletionEvent":entry.deletion_event},
                "assetRevision":entry.asset_revision, "masterRevision":entry.master_revision,
            },
        })
    }
    fn save_capture(&self, capture: &serde_json::Value) {
        // Issued evidence is compared byte-for-byte when it is retired. Match
        // the production serializer's field order for this restart fixture.
        let fields = [
            "id",
            "job",
            "config",
            "entry",
            "generation",
            "phase",
            "versions",
            "dirty",
            "photo",
            "master",
            "content_generation",
            "body",
            "preparation",
        ];
        let encoded = format!(
            "{{{}}}",
            fields
                .iter()
                .map(|key| format!("\"{key}\":{}", capture[key]))
                .collect::<Vec<_>>()
                .join(",")
        );
        self.pool.get().unwrap().execute(
            "INSERT INTO sync_tasks(operation_id,job_id,entry_id,action,state,config_generation,entry_generation,native_capture) VALUES(?1,?2,?3,'modify','issued',?4,?5,?6)",
            rusqlite::params![capture["id"].as_str(), self.job.id, capture["entry"].as_i64(), self.job.config_generation, capture["versions"][0].as_i64(), encoded],
        ).unwrap();
    }
}

#[test]
fn pending_asset_failure_does_not_block_other_photos_and_retries_next_cycle() {
    let server = Server::start().expect("upload failure tests require SyncServer");
    block_on(async {
        let f = Fixture::new(server).await;
        for media in db::list_all_media(&f.pool).unwrap() {
            common::write_distinct_jpeg(&media.path, 96, 80, [220, 30, 60]);
            f.actor
                .execute_blocking(DbCommand::UpdateEditedMedia {
                    id: MediaId::from(media.id),
                    item: Fixture::item(&media.path),
                })
                .unwrap();
        }
        let entry = f.service.store().uplink_view(f.job.id).unwrap().remove(0);
        let folder = f.root.path().join("staged");
        let snapshot = snapshot_source(
            &f.source.join(&entry.relative_path),
            &folder,
            "original",
            16_777_216,
        )
        .unwrap();
        let (thumbnail, digest) =
            photo_viewer::core::sync::content::thumbnail_from(&snapshot, 2048, 1_048_576).unwrap();
        let thumbnail_path = folder.join("thumbnail.jpg");
        std::fs::write(&thumbnail_path, thumbnail).unwrap();
        let saved = folder.join("temporarily-unavailable");
        std::fs::rename(&snapshot.path, &saved).unwrap();
        let mut capture = f.capture(&entry);
        capture["preparation"]["resources"] = json!({
            "original":{"path":snapshot.path,"hex":snapshot.digest.hex,"base64":snapshot.digest.base64,"size":snapshot.digest.size},
            "thumbnail":{"path":thumbnail_path,"hex":digest.hex,"base64":digest.base64,"size":digest.size},
        });
        f.save_capture(&capture);
        let first = f.service.run_cycle(f.job.id).await.unwrap();
        assert_eq!(first.failures.len(), 1, "{first:?}");
        assert_eq!(first.failures[0].entry_id, entry.entry_id);
        assert_eq!(first.mutations_confirmed, 2, "{first:?}");
        assert_eq!(first.outstanding, 1, "{first:?}");
        assert!(first.released, "{first:?}");
        assert_eq!(
            f.failure(entry.entry_id).as_deref(),
            Some(first.failures[0].reason.as_str())
        );

        // A persistent bad recovery item must not starve newly discovered work.
        f.add(3..4);
        let second = f.service.run_cycle(f.job.id).await.unwrap();
        assert_eq!(second.failures.len(), 1, "{second:?}");
        assert_eq!(second.mutations_confirmed, 1, "{second:?}");
        assert_eq!(second.outstanding, 1, "{second:?}");
        assert!(second.released, "{second:?}");
        let pending: (i64, String) = f
            .pool
            .get()
            .unwrap()
            .query_row(
                "SELECT count(*),operation_id FROM sync_tasks WHERE state='issued'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(pending, (1, capture["id"].as_str().unwrap().into()));

        // Restoring the same snapshot lets the original logical operation finish.
        std::fs::rename(saved, &snapshot.path).unwrap();
        let third = f.service.run_cycle(f.job.id).await.unwrap();
        assert!(third.failures.is_empty(), "{third:?}");
        assert_eq!(third.mutations_confirmed, 1, "{third:?}");
        assert_eq!(third.outstanding, 0, "{third:?}");
        assert!(f.failure(entry.entry_id).is_none());
        let state: String = f
            .pool
            .get()
            .unwrap()
            .query_row(
                "SELECT state FROM sync_tasks WHERE operation_id=?1",
                [capture["id"].as_str().unwrap()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(state, "confirmed");
    });
}

#[test]
fn rejected_pending_metadata_is_not_reissued_in_the_same_cycle() {
    let server = Server::start().expect("upload failure tests require SyncServer");
    block_on(async {
        let f = Fixture::new(server).await;
        f.actor
            .execute_blocking(DbCommand::SetFavorite {
                ids: db::list_all_media(&f.pool)
                    .unwrap()
                    .iter()
                    .map(|m| MediaId::from(m.id))
                    .collect(),
                is_favorite: true,
            })
            .unwrap();
        let entry = f.service.store().uplink_view(f.job.id).unwrap().remove(0);
        let mut capture = f.capture(&entry);
        capture["body"] = json!({
            "zoneID":{"zoneName":"PrimarySync","ownerRecordName":"_defaultOwner"},"atomic":true,
            "operations":[{"operationType":"update","record":{
                "recordName":entry.photo_id,"recordType":"CPLAsset","recordChangeTag":"stale-change-tag",
                "fields":capture["preparation"]["metadata"],"_syncserver":capture["preparation"]["events"],
            }}],
        });
        f.save_capture(&capture);
        let first = f.service.run_cycle(f.job.id).await.unwrap();
        assert_eq!(first.failures.len(), 1, "{first:?}");
        assert_eq!(first.mutations_confirmed, 2, "{first:?}");
        assert_eq!(first.outstanding, 1, "{first:?}");
        assert!(f.failure(entry.entry_id).is_some());
        let tasks: i64 = f
            .pool
            .get()
            .unwrap()
            .query_row(
                "SELECT count(*) FROM sync_tasks WHERE entry_id=?1 AND action='modify'",
                [entry.entry_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            tasks, 1,
            "a rejected photo must not get another operation in this cycle"
        );
        let next = f.service.run_cycle(f.job.id).await.unwrap();
        assert!(next.failures.is_empty(), "{next:?}");
        assert_eq!(next.mutations_confirmed, 1, "{next:?}");
        assert_eq!(next.outstanding, 0, "{next:?}");
        assert!(f.failure(entry.entry_id).is_none());
    });
}

#[test]
fn a_new_photo_preparation_failure_is_saved_and_does_not_block_later_photos() {
    let server = Server::start().expect("upload failure tests require SyncServer");
    block_on(async {
        let f = Fixture::new(server).await;
        f.add(3..6);
        let broken = f.source.join("photo-3.jpg");
        std::fs::write(&broken, b"not an image").unwrap();
        let first = f.service.run_cycle(f.job.id).await.unwrap();
        assert_eq!(first.failures.len(), 1, "{first:?}");
        assert_eq!(first.mutations_confirmed, 2, "{first:?}");
        assert_eq!(first.outstanding, 1, "{first:?}");
        let entry = first.failures[0].entry_id;
        assert_eq!(
            f.failure(entry).as_deref(),
            Some(first.failures[0].reason.as_str())
        );
        common::write_distinct_jpeg(&broken, 64, 48, [40, 180, 90]);
        let next = f.service.run_cycle(f.job.id).await.unwrap();
        assert!(next.failures.is_empty(), "{next:?}");
        assert_eq!(next.mutations_confirmed, 1, "{next:?}");
        assert_eq!(next.outstanding, 0, "{next:?}");
        assert!(f.failure(entry).is_none());
    });
}

#[test]
fn a_receipt_confirmation_failure_is_skipped_until_the_next_cycle() {
    let server = Server::start().expect("upload failure tests require SyncServer");
    block_on(async {
        let f = Fixture::new(server).await;
        f.actor
            .execute_blocking(DbCommand::SetFavorite {
                ids: db::list_all_media(&f.pool)
                    .unwrap()
                    .iter()
                    .map(|m| MediaId::from(m.id))
                    .collect(),
                is_favorite: true,
            })
            .unwrap();
        let entry = f.service.store().uplink_view(f.job.id).unwrap().remove(0);
        let mut capture = f.capture(&entry);
        capture["body"] = json!({
            "zoneID":{"zoneName":"PrimarySync","ownerRecordName":"_defaultOwner"}, "atomic":true,
            "operations":[{"operationType":"update","record":{
                "recordName":entry.photo_id, "recordType":"CPLAsset", "recordChangeTag":entry.asset_revision,
                "fields":capture["preparation"]["metadata"], "_syncserver":capture["preparation"]["events"],
            }}],
        });
        // The server committed before the application stopped. The next run
        // must settle this exact receipt without inventing another operation.
        let client = f._server.client();
        let scope = client.probe().await.unwrap();
        let zone = NativeZone::new("PrimarySync");
        let (grant, started) = client
            .acquire_lease("receipt-recovery", &zone)
            .await
            .unwrap();
        let lease = UploadLease::from_grant(&grant, started);
        let token = client.snapshot(&zone).await.unwrap().sync_token;
        assert!(
            client
                .check_lease(lease.proof(), &zone, &token, true)
                .await
                .unwrap()
                .may_upload
        );
        client
            .set_phase(lease.proof(), UploadPhase::Modify)
            .await
            .unwrap();
        let mut url = client.cloudkit_base(&scope.namespace).unwrap();
        url.path_segments_mut()
            .unwrap()
            .pop_if_empty()
            .push("records")
            .push("modify");
        let mut request = reqwest::Client::new()
            .post(url)
            .bearer_auth(&f._server.api_key)
            .header("Content-Type", "application/json")
            .header("Idempotency-Key", capture["id"].as_str().unwrap())
            .body(serde_json::to_vec(&capture["body"]).unwrap());
        for (name, value) in photo_viewer::core::sync::native::lease::lease_headers(lease.proof()) {
            request = request.header(name, value);
        }
        let response = request.send().await.unwrap();
        assert!(response.status().is_success());
        let response: serde_json::Value =
            serde_json::from_slice(&response.bytes().await.unwrap()).unwrap();
        assert!(
            response["records"].as_array().is_some_and(
                |records| records.len() == 1 && records[0].get("serverErrorCode").is_none()
            ),
            "{response}"
        );
        client.release_lease(lease.proof()).await.unwrap();
        f.save_capture(&capture);
        f.pool.get().unwrap().execute_batch(&format!(
            "CREATE TRIGGER reject_receipt_confirmation BEFORE UPDATE OF state ON sync_tasks WHEN NEW.entry_id={} AND NEW.state='confirmed' BEGIN SELECT RAISE(ABORT,'injected receipt confirmation failure'); END;",
            entry.entry_id,
        )).unwrap();
        let first = f.service.run_cycle(f.job.id).await.unwrap();
        assert_eq!(first.failures.len(), 1, "{first:?}");
        assert_eq!(first.failures[0].entry_id, entry.entry_id);
        assert_eq!(first.mutations_confirmed, 2, "{first:?}");
        assert_eq!(first.outstanding, 1, "{first:?}");
        assert!(f
            .failure(entry.entry_id)
            .unwrap()
            .contains("injected receipt confirmation failure"));
        let state: String = f
            .pool
            .get()
            .unwrap()
            .query_row(
                "SELECT state FROM sync_tasks WHERE operation_id=?1",
                [capture["id"].as_str().unwrap()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(state, "issued");
        f.pool
            .get()
            .unwrap()
            .execute_batch("DROP TRIGGER reject_receipt_confirmation")
            .unwrap();
        let next = f.service.run_cycle(f.job.id).await.unwrap();
        assert!(next.failures.is_empty(), "{next:?}");
        assert_eq!(next.mutations_confirmed, 1, "{next:?}");
        assert_eq!(
            next.mutations_sent, 0,
            "a committed operation must not be resent"
        );
        assert_eq!(next.outstanding, 0, "{next:?}");
        assert!(f.failure(entry.entry_id).is_none());
    });
}
