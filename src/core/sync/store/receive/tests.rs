//! Durable receive state: the bootstrap markers, the ordered inbox, and the
//! rule that `applied` never runs ahead of settled work.
//!
//! These are the C1.2 acceptance cases that do not need a server. The
//! real-server half (concurrent writes at a fixed height, a restart mid-page,
//! token expiry) lives in `tests/native_sync_receive.rs`.

use super::*;
use crate::core::sync::native::demo_pinned_scope;
use crate::core::sync::native::receive::{ZoneId, SUPPORTED_RECORD_TYPES};
use crate::core::sync::native::records::{
    AssetDescriptor, ProjectedRecord, RemoteGraph, RemotePhoto,
};
use crate::core::sync::store::tests::{new_job, temp_root};
use crate::core::sync::SyncJob;

/// Promotion with nothing measured. These cases exercise bookkeeping; a measured
/// digest is what would let an entry adopt a cloud identity instead.
fn no_local_digests(
) -> std::collections::HashMap<String, crate::core::sync::store::receive::LocalDigest> {
    std::collections::HashMap::new()
}

fn namespace() -> NativeNamespace {
    demo_pinned_scope("https://sync.example.invalid/", &["PrimarySync"]).namespace
}

fn zone() -> NativeZone {
    NativeZone::new("PrimarySync")
}

fn photo(id: &str, generation: &str, path: &str) -> RemotePhoto {
    RemotePhoto {
        photo_id: id.into(),
        master_id: format!("master-{id}"),
        content_generation: generation.into(),
        relative_path: path.into(),
        is_favorite: false,
        asset_date_ms: Some(1_700_000_000_000),
        is_hidden: false,
        original: AssetDescriptor {
            asset_id: format!("asset-original-{id}"),
            size: 2048,
            file_checksum: "47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU=".into(),
            mime_type: "image/jpeg".into(),
            width: Some(800),
            height: Some(600),
        },
        thumbnail: AssetDescriptor {
            asset_id: format!("asset-thumb-{id}"),
            size: 512,
            file_checksum: "47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU=".into(),
            mime_type: "image/jpeg".into(),
            width: Some(200),
            height: Some(150),
        },
        thumbnail_source_sha256: "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
            .into(),
        events: Default::default(),
    }
}

fn graph(photos: Vec<RemotePhoto>) -> RemoteGraph {
    RemoteGraph {
        photos,
        records: Vec::new(),
        tombstones: Vec::new(),
    }
}

fn store_with_job(name: &str) -> (SyncStore, crate::core::db::DbPool, SyncJob) {
    let pool = crate::core::db::init_pool(&temp_root(name).join("library.db")).unwrap();
    let store = SyncStore::new(pool.clone());
    let job = store
        .create_job(&new_job(temp_root(&format!("{name}-media"))))
        .unwrap();
    (store, pool, job)
}

fn begin(store: &SyncStore, job: &SyncJob, scope: &str) -> NativeProgress {
    store
        .begin_native_receive(
            job.id,
            namespace(),
            "501855bb-18d3-4008-abc3-37e4948bf6f0",
            scope,
            zone(),
        )
        .unwrap()
}

#[test]
fn a_first_receive_starts_with_no_position_at_all() {
    let (store, _pool, job) = store_with_job("receive-fresh");
    assert!(store.native_progress(job.id).unwrap().is_none());

    let progress = begin(&store, &job, "scope-1");
    assert!(progress.snapshot_token.is_none());
    assert!(progress.ingest_token.is_none());
    assert!(progress.applied_token.is_none());
    assert!(progress.target_token.is_none());
    assert!(!progress.bootstrap.started);
    assert!(progress.needs_bootstrap(&SUPPORTED_RECORD_TYPES));
}

#[test]
fn a_bootstrap_is_incomplete_until_every_supported_type_finished() {
    let (store, _pool, job) = store_with_job("receive-bootstrap");
    begin(&store, &job, "scope-1");
    store
        .pin_native_snapshot(job.id, "snapshot.h", "zone.h")
        .unwrap();

    // One type finished is still not an inventory.
    let progress = store
        .save_bootstrap_page(job.id, SUPPORTED_RECORD_TYPES[0], None, 3, "zone.h")
        .unwrap();
    assert!(progress.needs_bootstrap(&SUPPORTED_RECORD_TYPES));
    assert_eq!(
        progress.bootstrap.marker(SUPPORTED_RECORD_TYPES[0]).status,
        TypeStatus::Complete
    );

    // A type with a continuation marker is explicitly still pending, so a
    // restart resumes its page instead of restarting the whole read.
    let progress = store
        .save_bootstrap_page(
            job.id,
            SUPPORTED_RECORD_TYPES[1],
            Some("query.next".into()),
            1,
            "zone.h",
        )
        .unwrap();
    assert_eq!(
        progress.bootstrap.marker(SUPPORTED_RECORD_TYPES[1]).status,
        TypeStatus::Pending
    );
    assert_eq!(
        progress
            .bootstrap
            .marker(SUPPORTED_RECORD_TYPES[1])
            .continuation_marker,
        Some("query.next".into())
    );
    assert!(progress.needs_bootstrap(&SUPPORTED_RECORD_TYPES));

    let progress = store
        .save_bootstrap_page(job.id, SUPPORTED_RECORD_TYPES[1], None, 2, "zone.h")
        .unwrap();
    assert!(!progress.needs_bootstrap(&SUPPORTED_RECORD_TYPES));
    // The snapshot position the read established is kept, not re-minted.
    assert_eq!(progress.snapshot_token.as_deref(), Some("snapshot.h"));
}

#[test]
fn a_changed_scope_invalidates_every_previous_completeness_claim() {
    let (store, pool, job) = store_with_job("receive-scope");
    begin(&store, &job, "scope-1");
    for record_type in SUPPORTED_RECORD_TYPES {
        store
            .save_bootstrap_page(job.id, record_type, None, 1, "zone.h")
            .unwrap();
    }
    store.save_delta_page(job.id, Vec::new(), "zone.h").unwrap();
    // An empty zone owes nothing, so this round really is caught up and the
    // applied position is granted.
    let (_, _, _, applied) = store
        .promote_remote_graph(job.id, RemoteGraph::default(), "zone.h", no_local_digests())
        .unwrap();
    assert!(applied);
    assert_eq!(
        store
            .native_progress(job.id)
            .unwrap()
            .unwrap()
            .applied_token
            .as_deref(),
        Some("zone.h")
    );

    // Widening the scope must not inherit the old completeness claim: the newly
    // included resources were never downloaded.
    let progress = begin(&store, &job, "scope-2");
    assert!(progress.snapshot_token.is_none());
    assert!(progress.target_token.is_none());
    assert!(progress.applied_token.is_none());
    assert!(progress.needs_bootstrap(&SUPPORTED_RECORD_TYPES));
    let inbox: i64 = pool
        .get()
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM sync_native_inbox WHERE job_id=?1",
            [job.id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(inbox, 0);
}

#[test]
fn a_changed_database_generation_also_invalidates_the_position() {
    let (store, _pool, job) = store_with_job("receive-generation");
    let progress = begin(&store, &job, "scope-1");
    assert_eq!(
        progress.database_generation,
        "501855bb-18d3-4008-abc3-37e4948bf6f0"
    );

    // A restored server backup mints a new generation. Every token from the old
    // one is meaningless, so the position must not survive it.
    let progress = store
        .begin_native_receive(
            job.id,
            namespace(),
            "11111111-2222-4333-8444-555555555555",
            "scope-1",
            zone(),
        )
        .unwrap();
    assert_eq!(
        progress.database_generation,
        "11111111-2222-4333-8444-555555555555"
    );
    assert!(progress.needs_bootstrap(&SUPPORTED_RECORD_TYPES));
}

#[test]
fn delta_events_are_ordered_deduplicated_and_never_skip_a_page() {
    let (store, pool, job) = store_with_job("receive-inbox");
    begin(&store, &job, "scope-1");

    let event = |ordinal: i64, name: &str, tag: &str| InboxEvent {
        ordinal,
        record_name: name.into(),
        record_type: "CPLAsset".into(),
        record_change_tag: tag.into(),
        kind: InboxKind::Updated,
        payload: serde_json::json!({ "recordName": name }),
    };

    let first = store
        .save_delta_page(
            job.id,
            vec![event(0, "a1", "tag-1"), event(0, "a2", "tag-2")],
            "zone.page-1",
        )
        .unwrap();
    assert_eq!(first, 2);

    // Replaying the same page after a crash must not duplicate: the inbox's own
    // uniqueness on (record, revision) is what makes replay safe.
    let replayed = store
        .save_delta_page(
            job.id,
            vec![event(0, "a1", "tag-1"), event(0, "a2", "tag-2")],
            "zone.page-1",
        )
        .unwrap();
    assert_eq!(replayed, 0);

    let saved = store
        .save_delta_page(job.id, vec![event(0, "a3", "tag-3")], "zone.page-2")
        .unwrap();
    assert_eq!(saved, 1);

    let events = store
        .inbox_events(job.id, "501855bb-18d3-4008-abc3-37e4948bf6f0")
        .unwrap();
    assert_eq!(events.len(), 3);
    // Ordinals are dense and ascending; the replayed page did not create a gap
    // or a second a1.
    assert_eq!(
        events
            .iter()
            .map(|event| event.record_name.as_str())
            .collect::<Vec<_>>(),
        vec!["a1", "a2", "a3"]
    );
    assert!(events
        .windows(2)
        .all(|pair| pair[0].ordinal < pair[1].ordinal));

    let token: String = pool
        .get()
        .unwrap()
        .query_row(
            "SELECT ingest_token FROM sync_native_progress WHERE job_id=?1",
            [job.id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(token, "zone.page-2");
}

#[test]
fn promotion_materializes_work_and_leaves_applied_behind_it() {
    let (store, _pool, job) = store_with_job("receive-promote");
    begin(&store, &job, "scope-1");

    let (photos, unknown, resources, applied) = store
        .promote_remote_graph(
            job.id,
            graph(vec![photo("a1", "gen-1", "camera/one.jpg")]),
            "zone.target",
            no_local_digests(),
        )
        .unwrap();
    assert_eq!((photos, unknown, resources), (1, 0, 3));
    // The photo's bytes have not been downloaded, so this client is not caught
    // up — reporting otherwise would claim a complete pair it never fetched.
    assert!(!applied);

    let progress = store.native_progress(job.id).unwrap().unwrap();
    assert_eq!(progress.target_token.as_deref(), Some("zone.target"));
    assert_eq!(progress.applied_token, None);
    assert_eq!(store.unresolved_resource_count(job.id).unwrap(), 3);

    // Explicitly asking to apply is refused for the same reason.
    assert!(store.advance_applied_token(job.id).is_err());
}

#[test]
fn verified_resources_alone_do_not_authorize_an_unpublished_photo() {
    let (store, pool, job) = store_with_job("receive-applied");
    begin(&store, &job, "scope-1");
    store
        .promote_remote_graph(
            job.id,
            graph(vec![photo("a1", "gen-1", "camera/one.jpg")]),
            "zone.target",
            no_local_digests(),
        )
        .unwrap();
    assert!(store.advance_applied_token(job.id).is_err());

    // Even forged resource completion cannot bypass the durable photo intent.
    // The positive path uses real downloads/publication in native_sync_assets.
    pool.get()
        .unwrap()
        .execute(
            "UPDATE sync_native_resources SET state='verified' WHERE job_id=?1",
            [job.id],
        )
        .unwrap();
    assert_eq!(store.unresolved_resource_count(job.id).unwrap(), 1);
    assert!(store.advance_applied_token(job.id).is_err());
    assert!(store
        .native_progress(job.id)
        .unwrap()
        .unwrap()
        .applied_token
        .is_none());
}

#[test]
fn an_empty_zone_promotes_cleanly_and_applies_immediately() {
    let (store, _pool, job) = store_with_job("receive-empty");
    begin(&store, &job, "scope-1");
    let (_, _, resources, applied) = store
        .promote_remote_graph(
            job.id,
            RemoteGraph::default(),
            "zone.target",
            no_local_digests(),
        )
        .unwrap();
    assert_eq!(resources, 0);
    // Nothing to download means nothing to wait for. An empty inbox alone is
    // not the reason: the reason is that the projection is complete *and* owes
    // nothing.
    assert!(applied);
    let progress = store.native_progress(job.id).unwrap().unwrap();
    assert_eq!(progress.applied_token.as_deref(), Some("zone.target"));
}

#[test]
fn an_open_conflict_also_blocks_the_applied_position() {
    let (store, pool, job) = store_with_job("receive-conflict");
    begin(&store, &job, "scope-1");
    store
        .promote_remote_graph(
            job.id,
            RemoteGraph::default(),
            "zone.target",
            no_local_digests(),
        )
        .unwrap();
    pool.get()
        .unwrap()
        .execute(
            "INSERT INTO sync_entries(job_id,relative_path) VALUES(?1,'a.jpg')",
            [job.id],
        )
        .unwrap();
    pool.get()
        .unwrap()
        .execute(
            "INSERT INTO sync_conflicts(job_id,entry_id,kind) VALUES(?1,1,'both_modified')",
            [job.id],
        )
        .unwrap();
    pool.get()
        .unwrap()
        .execute(
            "UPDATE sync_native_progress SET applied_token=NULL WHERE job_id=?1",
            [job.id],
        )
        .unwrap();

    assert_eq!(store.unresolved_resource_count(job.id).unwrap(), 1);
    assert!(store.advance_applied_token(job.id).is_err());
}

#[test]
fn the_projection_is_replaced_wholesale_not_merged_with_a_previous_one() {
    let (store, _pool, job) = store_with_job("receive-replace");
    begin(&store, &job, "scope-1");
    let generation = "501855bb-18d3-4008-abc3-37e4948bf6f0";

    store
        .promote_remote_graph(
            job.id,
            graph(vec![
                photo("a1", "gen-1", "camera/one.jpg"),
                photo("a2", "gen-1", "camera/two.jpg"),
            ]),
            "zone.t1",
            no_local_digests(),
        )
        .unwrap();
    assert_eq!(
        store
            .remote_graph(job.id, generation)
            .unwrap()
            .photo_count(),
        2
    );

    // The next round saw a1 deleted. Promotion reflects the round that just
    // completed; it does not accumulate across rounds.
    store
        .promote_remote_graph(
            job.id,
            graph(vec![photo("a2", "gen-1", "camera/two.jpg")]),
            "zone.t2",
            no_local_digests(),
        )
        .unwrap();
    let graph = store.remote_graph(job.id, generation).unwrap();
    assert_eq!(graph.photo_count(), 1);
    assert_eq!(graph.photos[0].photo_id, "a2");
}

#[test]
fn unknown_record_types_survive_promotion_and_are_never_counted_as_photos() {
    let (store, _pool, job) = store_with_job("receive-unknown");
    begin(&store, &job, "scope-1");
    let mut graph = graph(vec![photo("a1", "gen-1", "camera/one.jpg")]);
    graph.records.push(ProjectedRecord {
        record_name: "album-1".into(),
        record_type: "Album".into(),
        record_change_tag: "tag-album".into(),
        payload: serde_json::json!({ "recordName": "album-1" }),
    });
    let (photos, unknown, _, _) = store
        .promote_remote_graph(job.id, graph, "zone.target", no_local_digests())
        .unwrap();
    assert_eq!((photos, unknown), (1, 1));

    let read_back = store
        .remote_graph(job.id, "501855bb-18d3-4008-abc3-37e4948bf6f0")
        .unwrap();
    assert_eq!(read_back.photo_count(), 1);
    assert_eq!(read_back.records.len(), 1);
    assert_eq!(read_back.records[0].record_type, "Album");
    assert_eq!(read_back.unknown_record_types().get("Album"), Some(&1));
}

#[test]
fn a_reset_drops_the_cloud_position_without_touching_local_or_remote_state() {
    let (store, pool, job) = store_with_job("receive-reset");
    begin(&store, &job, "scope-1");
    for record_type in SUPPORTED_RECORD_TYPES {
        store
            .save_bootstrap_page(job.id, record_type, None, 1, "zone.h")
            .unwrap();
    }
    store
        .save_delta_page(job.id, Vec::new(), "zone.ingest")
        .unwrap();
    store
        .promote_remote_graph(
            job.id,
            graph(vec![photo("a1", "gen-1", "a.jpg")]),
            "zone.target",
            no_local_digests(),
        )
        .unwrap();
    pool.get()
        .unwrap()
        .execute(
            "UPDATE sync_entries SET native_dirty=2,native_photo_id='photo-1' WHERE job_id=?1 AND relative_path='a.jpg'",
            [job.id],
        )
        .unwrap();

    // Token expiry rebuilds metadata from a new snapshot. Local obligations and
    // confirmed identity survive: they are facts about the user's library, not
    // about a position on the server.
    store.reset_native_progress(job.id).unwrap();
    let progress = store.native_progress(job.id).unwrap().unwrap();
    assert!(progress.snapshot_token.is_none());
    assert!(progress.ingest_token.is_none());
    assert!(progress.target_token.is_none());
    assert!(progress.applied_token.is_none());
    assert!(progress.needs_bootstrap(&SUPPORTED_RECORD_TYPES));

    let entry: (i64, Option<String>) = pool
        .get()
        .unwrap()
        .query_row(
            "SELECT native_dirty, native_photo_id FROM sync_entries WHERE job_id=?1",
            [job.id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(entry, (2, Some("photo-1".into())));
}

#[test]
fn the_zone_identity_survives_a_round_trip_through_storage() {
    let (store, _pool, job) = store_with_job("receive-zone");
    let progress = begin(&store, &job, "scope-1");
    assert_eq!(progress.zone, zone());
    assert_eq!(progress.namespace, namespace());
    // The zone id the client sends must be exactly the one it stored, owner
    // included: the service rejects any other owner outright.
    assert_eq!(
        ZoneId::from(&progress.zone).owner_record_name,
        "_defaultOwner"
    );
}

#[test]
fn failed_or_staged_resources_never_authorize_applied_progress() {
    for failed in [true, false] {
        let (store, _pool, job) = store_with_job(if failed {
            "receive-failed-barrier"
        } else {
            "receive-staged-barrier"
        });
        begin(&store, &job, "scope-1");
        let projection = graph(vec![photo("barrier-photo", "generation-1", "barrier.jpg")]);
        store
            .promote_remote_graph(job.id, projection.clone(), "target-1", no_local_digests())
            .unwrap();
        for resource in store.outstanding_resources(job.id).unwrap() {
            if failed {
                store
                    .fail_resource(
                        job.id,
                        &resource.asset_id,
                        &resource.role,
                        "interrupted download",
                    )
                    .unwrap();
            } else {
                store
                    .stage_resource(
                        job.id,
                        &resource.asset_id,
                        &resource.role,
                        "/owned/staging/not-published",
                    )
                    .unwrap();
            }
        }
        let (_, _, _, applied) = store
            .promote_remote_graph(job.id, projection, "target-2", no_local_digests())
            .unwrap();
        assert!(!applied);
        assert!(store
            .native_progress(job.id)
            .unwrap()
            .unwrap()
            .applied_token
            .is_none());
        assert!(store.advance_applied_token(job.id).is_err());
    }
}

// ---------------------------------------------------------------------------
// Identity adoption
//
// A local file with no cloud identity is not a conflict until the bytes say the
// two disagree. These cases pin that, because the failure mode it replaces
// marked an entire library conflicted the first time a client met a cloud it
// had published from another installation.
// ---------------------------------------------------------------------------

/// A photo whose original is the empty file, so its descriptor digest is the
/// well-known SHA-256 of nothing and a real local file can be made to match.
fn empty_photo(id: &str, path: &str) -> RemotePhoto {
    let mut photo = photo(id, "gen-1", path);
    photo.original.size = 0;
    photo
}

/// Register a local file the job covers, the way enrolment does: an entry with
/// no cloud identity, marked as a first publication.
fn enrol_local_file(pool: &crate::core::db::DbPool, job: &SyncJob, path: &str) -> i64 {
    let root = job.local_root.clone();
    let file = root.join(path);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, b"").unwrap();
    let conn = pool.get().unwrap();
    conn.execute(
        "INSERT INTO media_items (uri, path, folder_path, mime_type, file_mtime, file_size, blake3_hash, indexed_at)
         VALUES (?1, ?2, '', 'image/jpeg', 0, 0, '', 0)",
        rusqlite::params![
            format!("file://{}", file.display()),
            file.display().to_string()
        ],
    )
    .unwrap();
    let media: i64 = conn
        .query_row(
            "SELECT id FROM media_items WHERE path = ?1",
            [file.display().to_string()],
            |row| row.get(0),
        )
        .unwrap();
    conn.execute(
        "INSERT INTO sync_entries (job_id, media_id, relative_path, native_dirty) VALUES (?1, ?2, ?3, 1)",
        rusqlite::params![job.id, media, path],
    )
    .unwrap();
    conn.last_insert_rowid()
}

/// Give the inbox the record revisions adoption binds against.
fn seed_inbox_revisions(pool: &crate::core::db::DbPool, job: &SyncJob, photo: &RemotePhoto) {
    let conn = pool.get().unwrap();
    for (ordinal, name, tag) in [
        (0, photo.photo_id.as_str(), "tag-asset-1"),
        (1, photo.master_id.as_str(), "tag-master-1"),
    ] {
        conn.execute(
            "INSERT INTO sync_native_inbox
                (job_id, generation, zone_name, owner, ordinal, record_name, record_type, record_change_tag, kind, payload)
             VALUES (?1, ?2, 'PrimarySync', '_defaultOwner', ?3, ?4, 'CPLAsset', ?5, 'updated', NULL)",
            rusqlite::params![job.id, "501855bb-18d3-4008-abc3-37e4948bf6f0", ordinal, name, tag],
        )
        .unwrap();
    }
}

fn digests_of(
    store: &SyncStore,
    job: i64,
    path: &str,
) -> std::collections::HashMap<String, LocalDigest> {
    let entry = store
        .unclaimed_local_entries(job)
        .unwrap()
        .into_iter()
        .find(|entry| entry.relative_path == path);
    let mut map = std::collections::HashMap::new();
    if let Some(entry) = entry {
        map.insert(
            path.to_string(),
            LocalDigest {
                entry_id: entry.entry_id,
                local_version: entry.local_version,
                digest: entry.cached.unwrap_or_else(|| ContentDigest::of(b"")),
            },
        );
    }
    map
}

#[test]
fn a_never_published_file_adopts_the_cloud_photo_it_is_proven_to_be() {
    let (store, pool, job) = store_with_job("adopt-identical");
    begin(&store, &job, "scope-1");
    let photo = empty_photo("a1", "camera/one.jpg");
    enrol_local_file(&pool, &job, "camera/one.jpg");
    seed_inbox_revisions(&pool, &job, &photo);

    let (_, _, _, applied) = store
        .promote_remote_graph(
            job.id,
            graph(vec![photo.clone()]),
            "zone.target",
            digests_of(&store, job.id, "camera/one.jpg"),
        )
        .unwrap();

    let conn = pool.get().unwrap();
    let (identity, master, revision, dirty): (Option<String>, Option<String>, Option<String>, i64) =
        conn.query_row(
            "SELECT native_photo_id, native_master_id, native_asset_revision, native_dirty
             FROM sync_entries WHERE job_id = ?1",
            [job.id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert_eq!(identity.as_deref(), Some("a1"));
    assert_eq!(master.as_deref(), Some("master-a1"));
    assert_eq!(revision.as_deref(), Some("tag-asset-1"));
    // The cloud satisfied the pending publication, so nothing is owed upward.
    assert_eq!(dirty, 0);

    // The identity match is a claim; the digest is the proof. It is kept, so a
    // second cycle does not re-read the library to reach the same conclusion.
    let cached: Option<String> = conn
        .query_row(
            "SELECT native_local_digest FROM sync_entries WHERE job_id = ?1",
            [job.id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        serde_json::from_str::<LocalDigest>(&cached.unwrap())
            .unwrap()
            .digest
            .hex,
        ContentDigest::of(b"").hex
    );

    // Nothing to download: the local file is the cloud original. Leaving the
    // obligations outstanding would be worse than a wasted download — while any
    // remains, the applied position cannot advance and the job reports itself
    // as synchronizing forever.
    assert_eq!(store.unresolved_resource_count(job.id).unwrap(), 0);
    let open: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sync_conflicts WHERE job_id = ?1 AND state = 'open'",
            [job.id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(open, 0);
    assert!(applied);

    // Both roles are settled and point at the library file, not at staging:
    // adoption stages nothing, because there is nothing to stage.
    let settled: Vec<(String, Option<String>)> = {
        let mut stmt = conn
            .prepare("SELECT role, verified_cache FROM sync_native_resources WHERE job_id = ?1 ORDER BY role")
            .unwrap();
        stmt.query_map([job.id], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
    };
    assert_eq!(settled.len(), 2, "adoption left a resource obligation open");
    for (role, cache) in &settled {
        assert!(
            cache
                .as_deref()
                .is_some_and(|value| value.starts_with("file://")),
            "{role} was settled without pointing at the library file: {cache:?}"
        );
    }
}

#[test]
fn adoption_keeps_a_local_metadata_edit_and_leaves_it_to_publish() {
    let (store, pool, job) = store_with_job("adopt-metadata");
    begin(&store, &job, "scope-1");
    let mut photo = empty_photo("a1", "camera/one.jpg");
    photo.is_favorite = true;
    let entry = enrol_local_file(&pool, &job, "camera/one.jpg");
    seed_inbox_revisions(&pool, &job, &photo);
    // The user favourited it after enrolment and before the cloud answered. That
    // is a real edit with no trustworthy cloud counterpart to weigh it against.
    pool.get()
        .unwrap()
        .execute(
            "UPDATE sync_entries SET native_dirty = native_dirty | 2,
                native_metadata_version = native_metadata_version + 1
             WHERE id = ?1",
            [entry],
        )
        .unwrap();
    pool.get()
        .unwrap()
        .execute(
            "UPDATE media_items SET is_favorite = 1 WHERE id = (SELECT media_id FROM sync_entries WHERE id = ?1)",
            [entry],
        )
        .unwrap();

    store
        .promote_remote_graph(
            job.id,
            graph(vec![photo.clone()]),
            "zone.target",
            digests_of(&store, job.id, "camera/one.jpg"),
        )
        .unwrap();

    let conn = pool.get().unwrap();
    let (identity, dirty): (Option<String>, i64) = conn
        .query_row(
            "SELECT native_photo_id, native_dirty FROM sync_entries WHERE id = ?1",
            [entry],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    // The identity is bound — the content question was settled — but the
    // metadata obligation survives as a conditional update.
    assert_eq!(identity.as_deref(), Some("a1"));
    assert_eq!(dirty, 2);

    let (favorite,): (i64,) = conn
        .query_row(
            "SELECT m.is_favorite FROM media_items m WHERE m.id = (SELECT media_id FROM sync_entries WHERE id = ?1)",
            [entry],
            |row| Ok((row.get(0)?,)),
        )
        .unwrap();
    // The cloud says favourite; the user said favourite too, but the point is
    // that the local value was not overwritten.
    assert_eq!(favorite, 1);
}

#[test]
fn a_local_edit_blocks_the_cloud_value_from_reaching_the_library_row() {
    let (store, pool, job) = store_with_job("adopt-metadata-prevails");
    begin(&store, &job, "scope-1");
    let mut photo = empty_photo("a1", "camera/one.jpg");
    photo.is_favorite = true;
    let entry = enrol_local_file(&pool, &job, "camera/one.jpg");
    seed_inbox_revisions(&pool, &job, &photo);
    pool.get()
        .unwrap()
        .execute(
            "UPDATE sync_entries SET native_dirty = native_dirty | 2,
                native_metadata_version = native_metadata_version + 1
             WHERE id = ?1",
            [entry],
        )
        .unwrap();
    // The user un-favourited it locally; the cloud has it favourited.
    pool.get()
        .unwrap()
        .execute(
            "UPDATE media_items SET is_favorite = 0 WHERE id = (SELECT media_id FROM sync_entries WHERE id = ?1)",
            [entry],
        )
        .unwrap();

    store
        .promote_remote_graph(
            job.id,
            graph(vec![photo.clone()]),
            "zone.target",
            digests_of(&store, job.id, "camera/one.jpg"),
        )
        .unwrap();

    let (favorite,): (i64,) = pool
        .get()
        .unwrap()
        .query_row(
            "SELECT m.is_favorite FROM media_items m WHERE m.id = (SELECT media_id FROM sync_entries WHERE id = ?1)",
            [entry],
            |row| Ok((row.get(0)?,)),
        )
        .unwrap();
    assert_eq!(
        favorite, 0,
        "the local metadata edit was overwritten by the cloud"
    );
}

#[test]
fn divergent_bytes_cannot_be_adopted_before_preserving_the_local_path() {
    let (store, pool, job) = store_with_job("adopt-divergent");
    begin(&store, &job, "scope-1");
    let mut photo = photo("a1", "gen-1", "camera/one.jpg");
    photo.original.size = 0;
    photo.original.file_checksum = crate::core::sync::content::to_base64(&[0xabu8; 32]);
    enrol_local_file(&pool, &job, "camera/one.jpg");
    seed_inbox_revisions(&pool, &job, &photo);

    store
        .promote_remote_graph(
            job.id,
            graph(vec![photo.clone()]),
            "zone.target",
            digests_of(&store, job.id, "camera/one.jpg"),
        )
        .unwrap();

    let conn = pool.get().unwrap();
    let identity: Option<String> = conn
        .query_row(
            "SELECT native_photo_id FROM sync_entries WHERE job_id = ?1",
            [job.id],
            |row| row.get(0),
        )
        .unwrap();
    assert!(identity.is_none(), "a divergent photo was adopted anyway");
    let reason: Option<String> = conn
        .query_row(
            "SELECT json_extract(native_detail, '$.reason') FROM sync_conflicts WHERE job_id = ?1 AND state = 'open'",
            [job.id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        reason.as_deref(),
        Some("cloud content at this path differs from the local file")
    );
    // The digest of the local file is a fact about the local file, so it is
    // kept even though the comparison failed. Without it, a permanently
    // conflicting photo would re-read its original on every cycle.
    let cached: Option<String> = conn
        .query_row(
            "SELECT native_local_digest FROM sync_entries WHERE job_id = ?1",
            [job.id],
            |row| row.get(0),
        )
        .unwrap();
    assert!(
        cached.is_some(),
        "a divergent entry discarded its measured digest"
    );
}

#[test]
fn a_hidden_cloud_photo_is_never_adopted() {
    let (store, pool, job) = store_with_job("adopt-hidden");
    begin(&store, &job, "scope-1");
    let mut photo = empty_photo("a1", "camera/one.jpg");
    photo.is_hidden = true;
    enrol_local_file(&pool, &job, "camera/one.jpg");
    seed_inbox_revisions(&pool, &job, &photo);

    store
        .promote_remote_graph(
            job.id,
            graph(vec![photo.clone()]),
            "zone.target",
            digests_of(&store, job.id, "camera/one.jpg"),
        )
        .unwrap();

    let conn = pool.get().unwrap();
    let identity: Option<String> = conn
        .query_row(
            "SELECT native_photo_id FROM sync_entries WHERE job_id = ?1",
            [job.id],
            |row| row.get(0),
        )
        .unwrap();
    assert!(identity.is_none());
    let reason: Option<String> = conn
        .query_row(
            "SELECT json_extract(native_detail, '$.reason') FROM sync_conflicts WHERE job_id = ?1 AND state = 'open'",
            [job.id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        reason.as_deref(),
        Some("remote hidden photo has no supported local representation")
    );
}

#[test]
fn an_issued_operation_defers_adoption_rather_than_deciding_it() {
    let (store, pool, job) = store_with_job("adopt-inflight");
    begin(&store, &job, "scope-1");
    let photo = empty_photo("a1", "camera/one.jpg");
    let entry = enrol_local_file(&pool, &job, "camera/one.jpg");
    seed_inbox_revisions(&pool, &job, &photo);
    // A create whose confirmation was lost. It still owns the entry, and
    // binding an identity underneath it would turn its recovery into a create
    // that can only come back 409.
    pool.get()
        .unwrap()
        .execute(
            "INSERT INTO sync_tasks (operation_id, job_id, entry_id, action, state, config_generation, entry_generation)
             VALUES ('op-1', ?1, ?2, 'upload', 'issued', 1, 1)",
            rusqlite::params![job.id, entry],
        )
        .unwrap();

    store
        .promote_remote_graph(
            job.id,
            graph(vec![photo.clone()]),
            "zone.target",
            digests_of(&store, job.id, "camera/one.jpg"),
        )
        .unwrap();

    let conn = pool.get().unwrap();
    let identity: Option<String> = conn
        .query_row(
            "SELECT native_photo_id FROM sync_entries WHERE id = ?1",
            [entry],
            |row| row.get(0),
        )
        .unwrap();
    assert!(
        identity.is_none(),
        "adoption overrode an in-flight operation"
    );
    // Nothing for the user to decide: this is a state that resolves itself.
    let open: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sync_conflicts WHERE entry_id = ?1 AND state = 'open'",
            [entry],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(open, 0);
}

#[test]
fn an_unchanged_projection_preserves_both_pending_categories() {
    let (store, pool, job) = store_with_job("merge-two-categories");
    begin(&store, &job, "scope-1");
    let photo = photo("a1", "gen-1", "camera/one.jpg");
    let entry = enrol_local_file(&pool, &job, "camera/one.jpg");
    seed_inbox_revisions(&pool, &job, &photo);
    // A confirmed baseline to compare against, so the projection is not the
    // photo's first appearance.
    pool.get()
        .unwrap()
        .execute(
            "UPDATE sync_entries SET native_photo_id = 'a1', native_master_id = 'master-a1',
                native_resource_baseline = ?2, native_dirty = 6,
                native_metadata_version = 2, native_content_version = 2
             WHERE id = ?1",
            rusqlite::params![entry, serde_json::to_string(&photo).unwrap()],
        )
        .unwrap();

    // Content and metadata are both pending — the state a mid-flight edit
    // produces. It is a bitmask, not a single value, so it must not read as
    // "some unconfirmed local edit" and become a conflict.
    store
        .promote_remote_graph(
            job.id,
            graph(vec![photo.clone()]),
            "zone.target",
            no_local_digests(),
        )
        .unwrap();

    let conn = pool.get().unwrap();
    let open: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sync_conflicts WHERE entry_id = ?1 AND state = 'open'",
            [entry],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(open, 0, "two pending categories were read as a conflict");
}

#[test]
fn a_cloud_photo_with_no_local_file_is_never_a_conflict() {
    let (store, pool, job) = store_with_job("adopt-no-local-file");
    begin(&store, &job, "scope-1");
    let photo = empty_photo("a1", "camera/one.jpg");
    seed_inbox_revisions(&pool, &job, &photo);
    // The entry exists and covers the path, but its media is gone: the user
    // removed it. There is nothing to compare against, and a conflict here
    // would block the download that would restore it.
    pool.get()
        .unwrap()
        .execute(
            "INSERT INTO sync_entries (job_id, relative_path, native_dirty) VALUES (?1, 'camera/one.jpg', 1)",
            [job.id],
        )
        .unwrap();

    store
        .promote_remote_graph(
            job.id,
            graph(vec![photo.clone()]),
            "zone.target",
            digests_of(&store, job.id, "camera/one.jpg"),
        )
        .unwrap();

    let conn = pool.get().unwrap();
    let open: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sync_conflicts WHERE job_id = ?1 AND state = 'open'",
            [job.id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(open, 0, "a missing local file was reported as a divergence");
    // Downloading is not enough: the pair must also have an admission intent.
    assert!(store.unresolved_resource_count(job.id).unwrap() > 0);
    let intents = store.pending_publications(job.id).unwrap();
    assert_eq!(
        intents.len(),
        1,
        "missing local photo has no admission intent"
    );
    assert_eq!(intents[0].photo_id, photo.photo_id);
}

#[test]
fn missing_local_media_does_not_bypass_an_issued_operation_or_delete_intent() {
    for owner in ["issued", "publishing", "delete"] {
        let (store, pool, job) = store_with_job(&format!("missing-local-owner-{owner}"));
        begin(&store, &job, "scope-1");
        let photo = empty_photo("a1", "camera/one.jpg");
        seed_inbox_revisions(&pool, &job, &photo);
        let conn = pool.get().unwrap();
        let dirty = if owner == "delete" { 9 } else { 1 };
        conn.execute(
            "INSERT INTO sync_entries(job_id,relative_path,native_dirty) VALUES(?1,'camera/one.jpg',?2)",
            rusqlite::params![job.id,dirty],
        ).unwrap();
        let entry = conn.last_insert_rowid();
        if owner == "issued" {
            conn.execute(
                "INSERT INTO sync_tasks(operation_id,job_id,entry_id,action,state,config_generation,entry_generation) VALUES('op-1',?1,?2,'upload','issued',1,1)",
                rusqlite::params![job.id,entry],
            ).unwrap();
        } else if owner == "publishing" {
            conn.execute(
                "INSERT INTO sync_publications(publication_id,job_id,entry_id,photo_id,state) VALUES('pub-1',?1,?2,?3,'publishing')",
                rusqlite::params![job.id,entry,photo.photo_id],
            ).unwrap();
        }
        drop(conn);
        store
            .promote_remote_graph(
                job.id,
                graph(vec![photo]),
                "zone.target",
                no_local_digests(),
            )
            .unwrap();
        let (identity, pending): (Option<String>, i64) = pool
            .get()
            .unwrap()
            .query_row(
                "SELECT native_photo_id,native_dirty FROM sync_entries WHERE id=?1",
                [entry],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert!(identity.is_none(), "{owner} lost ownership of the entry");
        assert_eq!(pending, dirty);
        let publications = store.pending_publications(job.id).unwrap();
        assert_eq!(publications.len(), usize::from(owner == "publishing"));
        if owner == "publishing" {
            assert_eq!(publications[0].publication_id, "pub-1");
            assert_eq!(publications[0].state, "publishing");
        }
    }
}

/// An entry created by a cloud projection has no `media_id`: nothing local
/// existed when it was first written, and enrolment's `ON CONFLICT DO NOTHING`
/// never backfills one. It is still a real entry for a real file, and adoption
/// has to be able to see it.
fn enrol_unlinked(pool: &crate::core::db::DbPool, job: &SyncJob, path: &str) -> i64 {
    let file = job.local_root.join(path);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, b"").unwrap();
    let conn = pool.get().unwrap();
    conn.execute(
        "INSERT INTO media_items (uri, path, folder_path, mime_type, file_mtime, file_size, blake3_hash, indexed_at)
         VALUES (?1, ?2, '', 'image/jpeg', 0, 0, '', 0)",
        rusqlite::params![format!("file://{}", file.display()), file.display().to_string()],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO sync_entries (job_id, relative_path, native_dirty) VALUES (?1, ?2, 1)",
        rusqlite::params![job.id, path],
    )
    .unwrap();
    conn.last_insert_rowid()
}

#[test]
fn an_entry_with_no_media_id_is_still_adopted_and_is_linked_by_adoption() {
    let (store, pool, job) = store_with_job("adopt-unlinked-entry");
    begin(&store, &job, "scope-1");
    let photo = empty_photo("a1", "camera/one.jpg");
    let entry = enrol_unlinked(&pool, &job, "camera/one.jpg");
    assert!(
        pool.get()
            .unwrap()
            .query_row(
                "SELECT media_id FROM sync_entries WHERE id = ?1",
                [entry],
                |r| r.get::<_, Option<i64>>(0)
            )
            .unwrap()
            .is_none(),
        "the fixture must start with no media association"
    );
    seed_inbox_revisions(&pool, &job, &photo);

    // The digests are supplied exactly as the receive path supplies them: the
    // file was found by path, not through media_id.
    store
        .promote_remote_graph(
            job.id,
            graph(vec![photo.clone()]),
            "zone.target",
            digests_of(&store, job.id, "camera/one.jpg"),
        )
        .unwrap();

    let conn = pool.get().unwrap();
    let (identity, media, dirty): (Option<String>, Option<i64>, i64) = conn
        .query_row(
            "SELECT native_photo_id, media_id, native_dirty FROM sync_entries WHERE id = ?1",
            [entry],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(identity.as_deref(), Some("a1"));
    assert!(
        media.is_some(),
        "adoption did not repair the missing association"
    );
    assert_eq!(dirty, 0);

    // And the store can now find it on its own, without being handed a digest.
    let found = store.unclaimed_local_entries(job.id).unwrap();
    assert!(
        found.is_empty(),
        "a bound entry is still offered for adoption"
    );
}

#[test]
fn adoption_of_unlinked_media_applies_cloud_metadata_or_keeps_local_dirty() {
    for local_edit in [false, true] {
        let (store, pool, job) = store_with_job(&format!("adopt-unlinked-metadata-{local_edit}"));
        begin(&store, &job, "scope-1");
        let mut photo = empty_photo("a1", "camera/one.jpg");
        photo.is_favorite = true;
        photo.asset_date_ms = Some(1_700_000_123_000);
        let entry = enrol_unlinked(&pool, &job, "camera/one.jpg");
        seed_inbox_revisions(&pool, &job, &photo);
        pool.get()
            .unwrap()
            .execute(
                "UPDATE media_items SET is_favorite=0,taken_at=42 WHERE path=?1",
                [job.local_root.join("camera/one.jpg").to_string_lossy()],
            )
            .unwrap();
        if local_edit {
            pool.get()
                .unwrap()
                .execute(
                    "UPDATE sync_entries SET native_dirty=3,native_metadata_version=1 WHERE id=?1",
                    [entry],
                )
                .unwrap();
        }

        // Exercise adoption directly, without the service's association backfill.
        // Repeating the projection must preserve the same metadata and obligation.
        for _ in 0..2 {
            store
                .promote_remote_graph(
                    job.id,
                    graph(vec![photo.clone()]),
                    "zone.target",
                    digests_of(&store, job.id, "camera/one.jpg"),
                )
                .unwrap();
            let (identity, favorite, taken, dirty, acked): (String, bool, i64, i64, i64) = pool.get().unwrap().query_row(
                "SELECT e.native_photo_id,m.is_favorite,m.taken_at,e.native_dirty,e.native_acked_metadata_version FROM sync_entries e JOIN media_items m ON m.id=e.media_id WHERE e.id=?1",
                [entry], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?)),
            ).unwrap();
            assert_eq!(identity, photo.photo_id);
            assert_eq!(favorite, !local_edit);
            assert_eq!(taken, if local_edit { 42 } else { 1_700_000_123 });
            assert_eq!(dirty, if local_edit { 2 } else { 0 });
            assert_eq!(acked, 0, "local metadata was acknowledged without upload");
            assert_eq!(store.unresolved_resource_count(job.id).unwrap(), 0);
            let uploads = store.uplink_view(job.id).unwrap();
            assert_eq!(uploads.len(), usize::from(local_edit));
            if local_edit {
                assert_eq!(uploads[0].dirty.bits(), 2);
                assert_eq!(uploads[0].photo_id.as_deref(), Some("a1"));
            }
        }
    }
}

#[test]
fn a_metadata_edit_after_adoption_remains_an_upload_obligation() {
    let (store, pool, job) = store_with_job("adopt-then-edit-metadata");
    begin(&store, &job, "scope-1");
    let photo = empty_photo("a1", "camera/one.jpg");
    let entry = enrol_unlinked(&pool, &job, "camera/one.jpg");
    seed_inbox_revisions(&pool, &job, &photo);
    store
        .promote_remote_graph(
            job.id,
            graph(vec![photo.clone()]),
            "zone.target",
            digests_of(&store, job.id, "camera/one.jpg"),
        )
        .unwrap();
    assert!(store.uplink_view(job.id).unwrap().is_empty());

    let mut conn = pool.get().unwrap();
    let tx = conn.transaction().unwrap();
    let media = tx
        .query_row(
            "SELECT media_id FROM sync_entries WHERE id=?1",
            [entry],
            |row| row.get(0),
        )
        .unwrap();
    tx.execute("UPDATE media_items SET is_favorite=1 WHERE id=?1", [media])
        .unwrap();
    SyncStore::note_local_change_in(
        &tx,
        media,
        crate::core::sync::store::ChangeCategory::Metadata,
        &crate::core::sync::store::EditEventStamp {
            edited_at_ms: 1_800_000_000_000,
            source: "local".into(),
            event_id: "after-adoption".into(),
        },
    )
    .unwrap();
    tx.commit().unwrap();
    drop(conn);
    store
        .promote_remote_graph(
            job.id,
            graph(vec![photo]),
            "zone.target",
            no_local_digests(),
        )
        .unwrap();
    let uploads = store.uplink_view(job.id).unwrap();
    assert_eq!(uploads.len(), 1);
    assert_eq!(uploads[0].photo_id.as_deref(), Some("a1"));
    assert_eq!(uploads[0].dirty.bits(), 2);
    assert!(crate::core::db::is_media_favorite(&pool, media).unwrap());
}

fn stamped_event(time: i64, source: &str) -> crate::core::sync::native::receive::EditEvent {
    crate::core::sync::native::receive::EditEvent {
        edited_at_ms: time,
        source: source.into(),
        event_id: format!("{source}-{time}"),
    }
}

fn note_edit(
    pool: &crate::core::db::DbPool,
    entry: i64,
    category: crate::core::sync::store::ChangeCategory,
    time: i64,
) {
    let mut conn = pool.get().unwrap();
    let tx = conn.transaction().unwrap();
    let media: i64 = tx
        .query_row(
            "SELECT media_id FROM sync_entries WHERE id=?1",
            [entry],
            |r| r.get(0),
        )
        .unwrap();
    SyncStore::note_local_change_in(
        &tx,
        media,
        category,
        &crate::core::sync::store::EditEventStamp {
            edited_at_ms: time,
            source: "local".into(),
            event_id: format!("local-{time}"),
        },
    )
    .unwrap();
    tx.commit().unwrap();
}

fn confirmed_local(
    name: &str,
) -> (
    SyncStore,
    crate::core::db::DbPool,
    SyncJob,
    i64,
    RemotePhoto,
) {
    let (store, pool, job) = store_with_job(name);
    begin(&store, &job, "scope-1");
    let baseline = empty_photo("a1", "camera/one.jpg");
    let entry = enrol_local_file(&pool, &job, "camera/one.jpg");
    seed_inbox_revisions(&pool, &job, &baseline);
    pool.get().unwrap().execute(
        "UPDATE sync_entries SET native_photo_id='a1',native_master_id='master-a1',native_dirty=0,native_resource_baseline=?2 WHERE id=?1",
        rusqlite::params![entry,serde_json::to_string(&baseline).unwrap()]).unwrap();
    (store, pool, job, entry, baseline)
}

fn assert_no_conflict(pool: &crate::core::db::DbPool, entry: i64) {
    let open: i64 = pool
        .get()
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM sync_conflicts WHERE entry_id=?1 AND state='open'",
            [entry],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        open, 0,
        "ordinary concurrent edits must resolve automatically"
    );
}

#[test]
fn adoption_cannot_confirm_an_edit_made_after_measurement() {
    use crate::core::sync::store::ChangeCategory;
    let (store, pool, job) = store_with_job("adopt-edit-after-measurement");
    begin(&store, &job, "scope-1");
    let remote = empty_photo("a1", "camera/one.jpg");
    let entry = enrol_local_file(&pool, &job, "camera/one.jpg");
    seed_inbox_revisions(&pool, &job, &remote);
    let measured = digests_of(&store, job.id, "camera/one.jpg");
    std::fs::write(job.local_root.join("camera/one.jpg"), b"new local content").unwrap();
    note_edit(&pool, entry, ChangeCategory::Content, 200);
    store
        .promote_remote_graph(job.id, graph(vec![remote]), "target", measured)
        .unwrap();
    let row:(i64,i64,Option<String>,Option<String>)=pool.get().unwrap().query_row(
        "SELECT native_dirty,native_acked_content_version,native_photo_id,native_local_digest FROM sync_entries WHERE id=?1",[entry],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).unwrap();
    assert_ne!(row.0 & 4, 0, "old digest must not clear the later edit");
    assert_eq!(row.1, 0);
    assert!(row.2.is_none());
    assert!(
        row.3.is_none(),
        "old digest must not be cached against the later version"
    );
}

#[test]
fn concurrent_metadata_edits_use_event_time_and_stable_ties() {
    use crate::core::sync::store::ChangeCategory;
    for (name, local_time, remote_time, remote_source, keep_local) in [
        ("local-newer", 300, 200, "remote", true),
        ("remote-newer", 100, 200, "remote", false),
        ("tie-local", 200, 200, "aaa", true),
        ("tie-remote", 200, 200, "zzz", false),
    ] {
        let (store, pool, job, entry, mut remote) =
            confirmed_local(&format!("lww-metadata-{name}"));
        pool.get().unwrap().execute("UPDATE media_items SET taken_at=42 WHERE id=(SELECT media_id FROM sync_entries WHERE id=?1)",[entry]).unwrap();
        note_edit(&pool, entry, ChangeCategory::Metadata, local_time);
        remote.asset_date_ms = Some(99_000);
        remote.events.metadata = Some(stamped_event(remote_time, remote_source));
        store
            .promote_remote_graph(job.id, graph(vec![remote]), "target", no_local_digests())
            .unwrap();
        assert_no_conflict(&pool, entry);
        let (dirty,taken,acked):(i64,i64,i64)=pool.get().unwrap().query_row("SELECT e.native_dirty,m.taken_at,e.native_acked_metadata_version FROM sync_entries e JOIN media_items m ON m.id=e.media_id WHERE e.id=?1",[entry],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap();
        assert_eq!(taken, if keep_local { 42 } else { 99 }, "{name}");
        assert_eq!(dirty, if keep_local { 2 } else { 0 }, "{name}");
        assert_eq!(acked, if keep_local { 0 } else { 1 }, "{name}");
        assert_eq!(
            store.unresolved_resource_count(job.id).unwrap(),
            0,
            "{name}"
        );
    }
}

#[test]
fn independent_local_content_and_remote_metadata_merge_without_downloads() {
    use crate::core::sync::store::ChangeCategory;
    let (store, pool, job, entry, mut remote) = confirmed_local("lww-independent");
    note_edit(&pool, entry, ChangeCategory::Content, 200);
    remote.is_favorite = true;
    remote.events.metadata = Some(stamped_event(300, "remote"));
    store
        .promote_remote_graph(job.id, graph(vec![remote]), "target", no_local_digests())
        .unwrap();
    assert_no_conflict(&pool, entry);
    let (dirty,favorite):(i64,bool)=pool.get().unwrap().query_row("SELECT e.native_dirty,m.is_favorite FROM sync_entries e JOIN media_items m ON m.id=e.media_id WHERE e.id=?1",[entry],|r|Ok((r.get(0)?,r.get(1)?))).unwrap();
    assert_eq!(dirty, 4);
    assert!(favorite);
    assert!(store.outstanding_resources(job.id).unwrap().is_empty());
    assert!(store.pending_publications(job.id).unwrap().is_empty());
    assert_eq!(store.unresolved_resource_count(job.id).unwrap(), 0);
}

#[test]
fn concurrent_content_and_metadata_choose_each_category_independently() {
    use crate::core::sync::store::ChangeCategory;
    for (
        name,
        local_content,
        remote_content,
        local_metadata,
        remote_metadata,
        content_wins,
        metadata_wins,
    ) in [
        ("both-local", 400, 200, 300, 100, true, true),
        ("both-remote", 100, 300, 200, 400, false, false),
        ("local-content", 400, 200, 100, 300, true, false),
        ("local-metadata", 100, 300, 400, 200, false, true),
    ] {
        let (store, pool, job, entry, mut remote) =
            confirmed_local(&format!("lww-categories-{name}"));
        note_edit(&pool, entry, ChangeCategory::Content, local_content);
        note_edit(&pool, entry, ChangeCategory::Metadata, local_metadata);
        pool.get().unwrap().execute("UPDATE media_items SET taken_at=42 WHERE id=(SELECT media_id FROM sync_entries WHERE id=?1)",[entry]).unwrap();
        remote.content_generation = "new-generation".into();
        remote.original.asset_id = "new-original".into();
        remote.thumbnail.asset_id = "new-thumb".into();
        remote.asset_date_ms = Some(99_000);
        remote.events.content = Some(stamped_event(remote_content, "remote"));
        remote.events.metadata = Some(stamped_event(remote_metadata, "remote"));
        store
            .promote_remote_graph(
                job.id,
                graph(vec![remote.clone()]),
                "target",
                no_local_digests(),
            )
            .unwrap();
        assert_no_conflict(&pool, entry);
        let dirty: i64 = pool
            .get()
            .unwrap()
            .query_row(
                "SELECT native_dirty FROM sync_entries WHERE id=?1",
                [entry],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            dirty,
            if content_wins { 4 } else { 0 } | if metadata_wins { 2 } else { 0 },
            "{name}"
        );
        if content_wins {
            assert!(
                store.pending_publications(job.id).unwrap().is_empty(),
                "{name}"
            );
            assert_eq!(
                store.unresolved_resource_count(job.id).unwrap(),
                0,
                "{name}"
            );
            // A repeated projection must continue to owe the newer local upload,
            // without reintroducing downloads of the remote losing version.
            store
                .promote_remote_graph(job.id, graph(vec![remote]), "next", no_local_digests())
                .unwrap();
            assert_eq!(store.unresolved_resource_count(job.id).unwrap(), 0);
        } else {
            let pending = store.pending_publications(job.id).unwrap();
            assert_eq!(pending.len(), 1, "{name}");
            let capture: String = pool
                .get()
                .unwrap()
                .query_row(
                    "SELECT captured_versions FROM sync_publications WHERE publication_id=?1",
                    [&pending[0].publication_id],
                    |r| r.get(0),
                )
                .unwrap();
            let capture: serde_json::Value = serde_json::from_str(&capture).unwrap();
            assert_eq!(capture["preserveMetadata"], metadata_wins, "{name}");
            assert_eq!(
                store.outstanding_resources(job.id).unwrap().len(),
                2,
                "{name}"
            );
        }
    }
}

#[test]
fn different_record_ids_do_not_use_the_timestamp_winner_rule() {
    use crate::core::sync::store::ChangeCategory;
    let (store, pool, job, entry, baseline) = confirmed_local("lww-requires-same-record-id");
    note_edit(&pool, entry, ChangeCategory::Metadata, 100);
    let mut other = baseline.clone();
    other.photo_id = "a-different-record".into();
    other.master_id = "a-different-master".into();
    other.is_favorite = true;
    other.events.metadata = Some(stamped_event(900, "remote"));
    seed_inbox_revisions(&pool, &job, &other);
    // Keep the original cloud record present too: this is an identity/path
    // disagreement, not remote deletion of the original record.
    let mut old = baseline;
    old.relative_path = "camera/other.jpg".into();
    store
        .promote_remote_graph(
            job.id,
            graph(vec![other, old]),
            "target",
            no_local_digests(),
        )
        .unwrap();
    let (identity,dirty,favorite):(String,i64,bool)=pool.get().unwrap().query_row("SELECT e.native_photo_id,e.native_dirty,m.is_favorite FROM sync_entries e JOIN media_items m ON m.id=e.media_id WHERE e.id=?1",[entry],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap();
    assert_eq!(identity, "a1");
    assert_eq!(dirty, 2);
    assert!(!favorite);
    let conflicts: i64 = pool
        .get()
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM sync_conflicts WHERE entry_id=?1 AND state='open'",
            [entry],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(conflicts, 1);
}

#[test]
fn a_cached_digest_is_also_scoped_to_the_measured_version() {
    use crate::core::sync::store::ChangeCategory;
    let (store, pool, job) = store_with_job("adopt-stale-cached-digest");
    begin(&store, &job, "scope-1");
    let remote = empty_photo("a1", "camera/one.jpg");
    let entry = enrol_local_file(&pool, &job, "camera/one.jpg");
    seed_inbox_revisions(&pool, &job, &remote);
    pool.get()
        .unwrap()
        .execute(
            "UPDATE sync_entries SET native_local_digest=?2 WHERE id=?1",
            rusqlite::params![
                entry,
                serde_json::to_string(&LocalDigest {
                    entry_id: entry,
                    local_version: 0,
                    digest: ContentDigest::of(b"")
                })
                .unwrap()
            ],
        )
        .unwrap();
    assert!(store.unclaimed_local_entries(job.id).unwrap()[0]
        .cached
        .is_some());
    let measured = digests_of(&store, job.id, "camera/one.jpg");
    std::fs::write(
        job.local_root.join("camera/one.jpg"),
        b"edited after cache lookup",
    )
    .unwrap();
    note_edit(&pool, entry, ChangeCategory::Content, 200);
    store
        .promote_remote_graph(job.id, graph(vec![remote]), "target", measured)
        .unwrap();
    let (dirty,acked,cached):(i64,i64,Option<String>)=pool.get().unwrap().query_row("SELECT native_dirty,native_acked_content_version,native_local_digest FROM sync_entries WHERE id=?1",[entry],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap();
    assert_ne!(dirty & 4, 0);
    assert_eq!(acked, 0);
    assert!(cached.is_none());
}

#[test]
fn remote_rename_can_restage_library_backed_resources_for_the_same_record() {
    let (store, pool, job, entry, mut remote) = confirmed_local("lww-rename-restages");
    store
        .promote_remote_graph(
            job.id,
            graph(vec![remote.clone()]),
            "old",
            no_local_digests(),
        )
        .unwrap();
    assert!(store.outstanding_resources(job.id).unwrap().is_empty());
    remote.relative_path = "camera/renamed.jpg".into();
    remote.events.metadata = Some(stamped_event(200, "remote"));
    store
        .promote_remote_graph(job.id, graph(vec![remote]), "new", no_local_digests())
        .unwrap();
    assert_no_conflict(&pool, entry);
    assert_eq!(store.pending_publications(job.id).unwrap().len(), 1);
    let resources = store.outstanding_resources(job.id).unwrap();
    assert_eq!(resources.len(), 2);
    assert!(resources
        .iter()
        .all(|resource| resource.state == super::super::ResourceState::Pending));
}

#[test]
fn a_replacement_entry_cannot_adopt_a_digest_of_the_previous_entry() {
    let (store, pool, job) = store_with_job("adopt-replaced-entry");
    begin(&store, &job, "scope-1");
    let remote = empty_photo("a1", "camera/one.jpg");
    let entry = enrol_local_file(&pool, &job, "camera/one.jpg");
    seed_inbox_revisions(&pool, &job, &remote);
    let measured = digests_of(&store, job.id, "camera/one.jpg");
    let media: i64 = pool
        .get()
        .unwrap()
        .query_row(
            "SELECT media_id FROM sync_entries WHERE id=?1",
            [entry],
            |r| r.get(0),
        )
        .unwrap();
    pool.get()
        .unwrap()
        .execute("DELETE FROM sync_entries WHERE id=?1", [entry])
        .unwrap();
    pool.get().unwrap().execute("INSERT INTO sync_entries(job_id,media_id,relative_path,native_dirty) VALUES(?1,?2,'camera/one.jpg',1)",rusqlite::params![job.id,media]).unwrap();
    store
        .promote_remote_graph(job.id, graph(vec![remote]), "target", measured)
        .unwrap();
    let (new_id, identity, dirty): (i64, Option<String>, i64) = pool
        .get()
        .unwrap()
        .query_row(
            "SELECT id,native_photo_id,native_dirty FROM sync_entries WHERE job_id=?1",
            [job.id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_ne!(new_id, entry);
    assert!(identity.is_none());
    assert_eq!(dirty, 1);
}

#[test]
fn missing_edit_events_have_a_deterministic_nonblocking_fallback() {
    use crate::core::sync::store::ChangeCategory;
    for (local_known, remote_known, keep_local) in [
        (false, false, true),
        (true, false, true),
        (false, true, false),
    ] {
        let (store, pool, job, entry, mut remote) =
            confirmed_local(&format!("lww-missing-{local_known}-{remote_known}"));
        note_edit(&pool, entry, ChangeCategory::Metadata, 100);
        if !local_known {
            pool.get()
                .unwrap()
                .execute(
                    "UPDATE sync_entries SET native_metadata_event=NULL WHERE id=?1",
                    [entry],
                )
                .unwrap();
        }
        pool.get().unwrap().execute("UPDATE media_items SET taken_at=42 WHERE id=(SELECT media_id FROM sync_entries WHERE id=?1)",[entry]).unwrap();
        remote.asset_date_ms = Some(99_000);
        if remote_known {
            remote.events.metadata = Some(stamped_event(200, "remote"));
        }
        store
            .promote_remote_graph(job.id, graph(vec![remote]), "target", no_local_digests())
            .unwrap();
        assert_no_conflict(&pool, entry);
        let (dirty,taken):(i64,i64)=pool.get().unwrap().query_row("SELECT e.native_dirty,m.taken_at FROM sync_entries e JOIN media_items m ON m.id=e.media_id WHERE e.id=?1",[entry],|r|Ok((r.get(0)?,r.get(1)?))).unwrap();
        assert_eq!(dirty, if keep_local { 2 } else { 0 });
        assert_eq!(taken, if keep_local { 42 } else { 99 });
    }
}

#[test]
fn adoption_does_not_settle_shared_assets_still_needed_by_another_photo() {
    let (store, pool, job) = store_with_job("adopt-shared-resources");
    begin(&store, &job, "scope-1");
    let first = empty_photo("a1", "camera/one.jpg");
    let mut second = first.clone();
    second.photo_id = "a2".into();
    second.master_id = "master-a2".into();
    second.relative_path = "camera/two.jpg".into();
    enrol_local_file(&pool, &job, "camera/one.jpg");
    seed_inbox_revisions(&pool, &job, &first);
    store
        .promote_remote_graph(
            job.id,
            graph(vec![first, second]),
            "target",
            digests_of(&store, job.id, "camera/one.jpg"),
        )
        .unwrap();
    assert_eq!(store.pending_publications(job.id).unwrap().len(), 1);
    let resources = store.outstanding_resources(job.id).unwrap();
    assert_eq!(resources.len(), 2);
    assert!(resources
        .iter()
        .all(|resource| resource.state == super::super::ResourceState::Pending));
}
