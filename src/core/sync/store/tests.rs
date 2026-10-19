use super::*;
use crate::core::sync::native::demo_pinned_scope as pinned;
use crate::core::sync::native::NativeZone;

pub(crate) fn new_job(root: PathBuf) -> NewSyncJob {
    NewSyncJob {
        provider_kind: ProviderKind::NativeSyncServer,
        endpoint: "https://sync.example.invalid/".into(),
        credential_ref: "credential-test".into(),
        local_root: root,
        direction: SyncDirection::Bidirectional,
        upload_scope: UploadScope::All,
        upload_albums: Vec::new(),
        native_zone: Some(NativeZone::new("PrimarySync")),
        native_pinned: Some(pinned("https://sync.example.invalid/", &["PrimarySync"])),
    }
}

pub(crate) fn temp_root(name: &str) -> PathBuf {
    crate::testing::Sandbox::install();
    let root = std::env::temp_dir().join(format!(
        "photoviewer-sync-store-{name}-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&root).expect("temp root");
    std::fs::canonicalize(&root).expect("canonical temp root")
}

#[test]
fn a_native_job_round_trips_its_zone_owner_and_pinned_scope() {
    let pool =
        crate::core::db::init_pool(&temp_root("native-round-trip").join("library.db")).unwrap();
    let store = SyncStore::new(pool.clone());
    let job = store
        .create_job(&new_job(temp_root("native-round-trip-media")))
        .unwrap();

    assert_eq!(job.provider_kind, ProviderKind::NativeSyncServer);
    assert_eq!(job.native_zone, Some(NativeZone::new("PrimarySync")));
    assert_eq!(job.credential_ref, "credential-test");

    // The zone lives in its own columns; there is no DAV collection path to
    // smuggle it through.
    let (zone, owner): (String, String) = pool
        .get()
        .unwrap()
        .query_row(
            "SELECT native_zone_name, native_owner FROM sync_jobs WHERE id=?1",
            [job.id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(zone, "PrimarySync");
    assert_eq!(owner, "_defaultOwner");

    let scope = store.native_scope(job.connection_id).unwrap().unwrap();
    assert_eq!(scope.public_base_url, "https://sync.example.invalid/");
    assert_eq!(scope.namespace.container, "com.apple.photos.cloud");
    assert_eq!(scope.protocol_version, 1);
    assert!(scope.has_zone("PrimarySync"));
}

#[test]
fn an_unprobed_connection_reports_no_native_scope() {
    let pool = crate::core::db::init_pool(&temp_root("unprobed-scope").join("library.db")).unwrap();
    let store = SyncStore::new(pool.clone());
    pool.get()
        .unwrap()
        .execute(
            "INSERT INTO sync_connections(id,provider_kind,endpoint,credential_ref)
             VALUES(1,'native_syncserver','https://sync.example.invalid/','ref')",
            [],
        )
        .unwrap();
    // A connection that never completed a probe has no pinned scope, which is
    // exactly the state in which a native job must not run.
    assert!(store.native_scope(1).unwrap().is_none());
}

#[test]
fn an_unprobed_or_mismatched_scope_is_rejected_before_any_row_is_written() {
    let pool = crate::core::db::init_pool(&temp_root("unprobed").join("library.db")).unwrap();
    let store = SyncStore::new(pool.clone());
    let count = |pool: &crate::core::db::DbPool| -> i64 {
        pool.get()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM sync_jobs", [], |row| row.get(0))
            .unwrap()
    };

    let mut unprobed = new_job(temp_root("unprobed-media"));
    unprobed.native_pinned = None;
    assert!(store.create_job(&unprobed).is_err());
    assert_eq!(count(&pool), 0);

    let mut unknown_zone = new_job(temp_root("unknown-zone-media"));
    unknown_zone.native_zone = Some(NativeZone::new("OtherSync"));
    assert!(store.create_job(&unknown_zone).is_err());
    assert_eq!(count(&pool), 0);

    // A scope probed against a different origin must not be reusable for this
    // endpoint: it would send the Bearer key somewhere the user never chose.
    let mut foreign = new_job(temp_root("foreign-origin-media"));
    foreign.native_pinned = Some(pinned(
        "https://elsewhere.example.invalid/",
        &["PrimarySync"],
    ));
    assert!(store.create_job(&foreign).is_err());
    assert_eq!(count(&pool), 0);

    let mut missing_zone = new_job(temp_root("missing-zone-media"));
    missing_zone.native_zone = None;
    assert!(store.create_job(&missing_zone).is_err());
    assert_eq!(count(&pool), 0);
}

#[test]
fn two_active_jobs_cannot_claim_the_same_zone_or_overlapping_roots() {
    let pool = crate::core::db::init_pool(&temp_root("zone-conflict").join("library.db")).unwrap();
    let store = SyncStore::new(pool.clone());
    let first = store
        .create_job(&new_job(temp_root("zone-conflict-a")))
        .unwrap();

    // A second connection to the same endpoint is still one server, so the
    // same zone must not fan out into two local roots even under a different
    // credential reference.
    let mut second = new_job(temp_root("zone-conflict-b"));
    second.credential_ref = "credential-test-two".into();
    second.native_pinned = Some(pinned("https://sync.example.invalid/", &["PrimarySync"]));
    assert!(store.create_job(&second).is_err());

    // A different zone is fine as long as the local roots do not overlap.
    let mut other_zone = new_job(temp_root("zone-conflict-c"));
    other_zone.credential_ref = "credential-test-three".into();
    other_zone.native_zone = Some(NativeZone::new("ArchiveSync"));
    other_zone.native_pinned = Some(pinned(
        "https://sync.example.invalid/",
        &["PrimarySync", "ArchiveSync"],
    ));
    let created = store.create_job(&other_zone).unwrap();
    assert_eq!(created.native_zone, Some(NativeZone::new("ArchiveSync")));

    // Nested local roots are ambiguous ownership and must be refused.
    let mut nested = new_job(first.local_root.join("nested"));
    nested.credential_ref = "credential-test-four".into();
    nested.native_zone = Some(NativeZone::new("ThirdSync"));
    nested.native_pinned = Some(pinned(
        "https://sync.example.invalid/",
        &["PrimarySync", "ThirdSync"],
    ));
    assert!(store.create_job(&nested).is_err());
}

#[test]
fn reusing_a_credential_reference_updates_one_connection_rather_than_duplicating_it() {
    let pool = crate::core::db::init_pool(&temp_root("reuse-ref").join("library.db")).unwrap();
    let store = SyncStore::new(pool.clone());
    store
        .create_job(&new_job(temp_root("reuse-ref-media")))
        .unwrap();

    // A second task with the same reference re-probes and refreshes the same
    // connection row; it must not leave two connections fighting over one
    // credential. A different zone is required so the task itself is valid.
    let mut reused = new_job(temp_root("reuse-ref-media-2"));
    reused.native_zone = Some(NativeZone::new("ArchiveSync"));
    reused.native_pinned = Some(pinned(
        "https://sync.example.invalid/",
        &["PrimarySync", "ArchiveSync"],
    ));
    store.create_job(&reused).unwrap();

    let connections: i64 = pool
        .get()
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM sync_connections WHERE credential_ref='credential-test'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(connections, 1);
}

#[test]
fn native_entry_columns_start_neutral_so_no_row_is_mistaken_for_a_dirtied_one() {
    let pool = crate::core::db::init_pool(&temp_root("neutral").join("library.db")).unwrap();
    let store = SyncStore::new(pool.clone());
    let job = store
        .create_job(&new_job(temp_root("neutral-media")))
        .unwrap();
    pool.get()
        .unwrap()
        .execute(
            "INSERT INTO sync_entries(job_id,relative_path) VALUES(?1,'a/photo.jpg')",
            [job.id],
        )
        .unwrap();

    let conn = pool.get().unwrap();
    let row: (i64, i64, Option<String>, Option<String>, Option<String>, i64) = conn
        .query_row(
            "SELECT native_dirty, native_local_version, native_photo_id, native_confirmed_generation,
                    native_metadata_source, native_local_version
             FROM sync_entries WHERE job_id=?1 AND relative_path='a/photo.jpg'",
            [job.id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?)),
        )
        .unwrap();
    assert_eq!(row.0, 0, "a new entry must be CLEAN");
    assert_eq!(row.1, 0);
    assert_eq!(row.2, None, "an allocated id must not precede a create");
    assert_eq!(row.3, None, "no confirmed pair exists yet");
    assert_eq!(
        row.4, None,
        "an unknown edit event stays NULL rather than being invented"
    );
    assert_eq!(row.5, 0);
}

#[test]
fn a_pending_edit_survives_and_merges_without_touching_the_active_configuration() {
    let pool = crate::core::db::init_pool(&temp_root("edit-first").join("library.db")).unwrap();
    let store = SyncStore::new(pool.clone());
    let job = store
        .create_job(&new_job(temp_root("edit-first-media")))
        .unwrap();

    assert!(store
        .queue_edit(job.id, JobEdit::UploadAlbums(vec!["a".into()]))
        .unwrap());
    // Re-submitting the same effective values is not a change.
    assert!(!store
        .queue_edit(job.id, JobEdit::UploadAlbums(vec!["a".into()]))
        .unwrap());
    // A different order is not a change either.
    assert!(!store
        .queue_edit(job.id, JobEdit::UploadAlbums(vec!["a".into()]))
        .unwrap());

    let pending = store.pending_change(job.id).unwrap().unwrap();
    assert_eq!(pending.upload_albums, Some(vec!["a".to_string()]));
    // The active configuration is untouched until the worker applies it.
    assert_eq!(
        store.get_job(job.id).unwrap().unwrap().upload_scope,
        UploadScope::All
    );
    let queued: i64 = pool
        .get()
        .unwrap()
        .query_row("SELECT COUNT(*) FROM sync_job_changes", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(queued, 1);
}

#[test]
fn a_job_without_a_zone_is_a_corrupt_row_rather_than_a_default() {
    let pool = crate::core::db::init_pool(&temp_root("zoneless").join("library.db")).unwrap();
    let store = SyncStore::new(pool.clone());
    store
        .create_job(&new_job(temp_root("zoneless-media")))
        .unwrap();
    // The database itself refuses a zoneless row: synthesising a zone would
    // point the task at a cloud location the user never chose.
    assert!(pool
        .get()
        .unwrap()
        .execute("UPDATE sync_jobs SET native_zone_name=NULL", [])
        .is_err());
    assert!(store.list_jobs().is_ok());
}

#[test]
fn startup_reconciliation_closes_runs_left_open_by_an_earlier_process() {
    let pool = crate::core::db::init_pool(&temp_root("reconcile").join("library.db")).unwrap();
    let store = SyncStore::new(pool.clone());
    let job = store
        .create_job(&new_job(temp_root("reconcile-media")))
        .unwrap();
    pool.get()
        .unwrap()
        .execute(
            "UPDATE sync_jobs SET last_started_at=100 WHERE id=?1",
            [job.id],
        )
        .unwrap();

    assert_eq!(store.reconcile_interrupted_runs().unwrap(), 1);
    // A second pass has nothing left to close, so it stays idempotent.
    assert_eq!(store.reconcile_interrupted_runs().unwrap(), 0);
}

#[test]
fn overview_remains_running_until_the_complete_inventory_is_synced() {
    let pool =
        crate::core::db::init_pool(&temp_root("overview-outstanding").join("library.db")).unwrap();
    let store = SyncStore::new(pool.clone());
    let job = store
        .create_job(&new_job(temp_root("overview-outstanding-media")))
        .unwrap();
    let conn = pool.get().unwrap();
    for (i, name) in ["a.jpg", "b.jpg"].iter().enumerate() {
        let path = job.local_root.join(name);
        conn.execute("INSERT INTO media_items(id,uri,path,folder_path,mime_type,file_mtime,file_size,blake3_hash,indexed_at,storage_location,thumbnail_ready) VALUES(?1,?2,?3,?4,'image/jpeg',1,1,'',1,3,1)",params![i as i64+1,crate::core::file_uri::from_path(&path),path.to_string_lossy(),job.local_root.to_string_lossy()]).unwrap();
        conn.execute("INSERT INTO sync_entries(job_id,media_id,relative_path,native_dirty,native_confirmed_generation) VALUES(?1,?2,?3,?4,?5)",params![job.id,i as i64+1,name,if i==0{0}else{1},if i==0{Some("generation")}else{None}]).unwrap();
    }
    conn.execute(
        "UPDATE sync_jobs SET last_started_at=100,last_completed_at=101 WHERE id=?1",
        [job.id],
    )
    .unwrap();

    let overview = store.overview_with_sync_enabled(true).unwrap();
    assert_eq!(overview.status, SyncOverviewStatus::Running);
    assert_eq!(overview.synced_items, 1);
    assert_eq!(overview.total_items, 2);

    conn.execute(
        "UPDATE sync_entries SET native_dirty=0,native_confirmed_generation='generation'
          WHERE job_id=?1 AND relative_path='b.jpg'",
        [job.id],
    )
    .unwrap();
    let complete = store.overview_with_sync_enabled(true).unwrap();
    assert_eq!(complete.status, SyncOverviewStatus::Completed);
    assert_eq!(complete.synced_items, 2);
    assert_eq!(complete.total_items, 2);

    conn.execute(
        "UPDATE sync_entries SET native_dirty=1
          WHERE job_id=?1 AND relative_path='a.jpg'",
        [job.id],
    )
    .unwrap();
    let edited = store.overview_with_sync_enabled(true).unwrap();
    assert_eq!(edited.status, SyncOverviewStatus::Running);
    assert_eq!(edited.synced_items, 1);
    assert_eq!(edited.total_items, 2);
}

#[test]
fn an_insecure_or_mislocated_endpoint_is_rejected() {
    for endpoint in [
        // A base path would silently prefix every control call.
        "https://sync.example.invalid/api/",
        // Cleartext needs a literal loopback address, not a DNS name.
        "http://sync.example.invalid/",
        "http://localhost:8790/",
        "https://user:pw@sync.example.invalid/",
    ] {
        let pool = crate::core::db::init_pool(
            &temp_root("endpoint").join(format!("{}.db", endpoint.len())),
        )
        .unwrap();
        let store = SyncStore::new(pool.clone());
        let mut request = new_job(temp_root("endpoint-media"));
        request.endpoint = endpoint.into();
        request.native_pinned = Some(pinned(endpoint, &["PrimarySync"]));
        assert!(
            store.create_job(&request).is_err(),
            "{endpoint} must be rejected as a native endpoint"
        );
        assert_eq!(
            pool.get()
                .unwrap()
                .query_row("SELECT COUNT(*) FROM sync_jobs", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
}

#[test]
fn upload_album_names_cannot_escape_the_local_root() {
    for albums in [
        vec!["../elsewhere".to_string()],
        vec!["a/../../b".to_string()],
        vec!["a//b".to_string()],
    ] {
        let pool = crate::core::db::init_pool(&temp_root("albums").join("library.db")).unwrap();
        let store = SyncStore::new(pool.clone());
        let mut request = new_job(temp_root("albums-media"));
        request.upload_albums = albums.clone();
        assert!(
            store.create_job(&request).is_err(),
            "{albums:?} must be rejected"
        );
    }
}

#[test]
fn overview_checks_publication_readiness_without_scanning_every_photo() {
    crate::testing::Sandbox::install();
    let pool =
        crate::core::db::init_pool(&temp_root("overview-publication-scale").join("library.db"))
            .unwrap();
    let store = SyncStore::new(pool.clone());
    let job = store
        .create_job(&new_job(temp_root("overview-publication-scale-media")))
        .unwrap();
    let mut conn = pool.get().unwrap();
    let plan = conn
        .prepare("EXPLAIN QUERY PLAN SELECT 1 FROM sync_publications p WHERE p.entry_id=?1 AND p.state NOT IN ('complete','superseded')")
        .unwrap()
        .query_map([1], |row| row.get::<_, String>(3))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert!(plan.iter().any(|step| step.contains("SEARCH p") && step.contains("entry_id=?")),
        "overview must look up one photo's publications rather than scan the entire table: {plan:?}");

    let tx = conn.transaction().unwrap();
    for id in 1..=4000 {
        let name = format!("photo-{id}.jpg");
        let path = job.local_root.join(&name);
        tx.execute("INSERT INTO media_items(id,uri,path,folder_path,mime_type,file_mtime,file_size,blake3_hash,indexed_at,storage_location,thumbnail_ready) VALUES(?1,?2,?3,?4,'image/jpeg',1,1,'',1,3,1)", params![id,crate::core::file_uri::from_path(&path),path.to_string_lossy(),job.local_root.to_string_lossy()]).unwrap();
        tx.execute("INSERT INTO sync_entries(id,job_id,media_id,relative_path,native_dirty,native_confirmed_generation) VALUES(?1,?2,?1,?3,0,'generation')", params![id,job.id,name]).unwrap();
        tx.execute("INSERT INTO sync_publications(publication_id,job_id,entry_id,state) VALUES(?1,?2,?3,?4)", params![format!("publication-{id}"),job.id,id,if id == 4000 { "pending" } else { "complete" }]).unwrap();
    }
    tx.commit().unwrap();
    drop(conn);
    let started = std::time::Instant::now();
    let overview = store.overview_with_sync_enabled(true).unwrap();
    assert_eq!(overview.total_items, 4000);
    assert_eq!(overview.synced_items, 3999);
    assert!(
        started.elapsed() < std::time::Duration::from_secs(1),
        "4,000-photo overview took {:?}",
        started.elapsed()
    );
}
