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
    assert_eq!(progress.snapshot_token.as_deref(), Some("zone.h"));
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
        .promote_remote_graph(job.id, RemoteGraph::default(), "zone.h")
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
        )
        .unwrap();
    assert_eq!((photos, unknown, resources), (1, 0, 2));
    // The photo's bytes have not been downloaded, so this client is not caught
    // up — reporting otherwise would claim a complete pair it never fetched.
    assert!(!applied);

    let progress = store.native_progress(job.id).unwrap().unwrap();
    assert_eq!(progress.target_token.as_deref(), Some("zone.target"));
    assert_eq!(progress.applied_token, None);
    assert_eq!(store.unresolved_resource_count(job.id).unwrap(), 2);

    // Explicitly asking to apply is refused for the same reason.
    assert!(store.advance_applied_token(job.id).is_err());
}

#[test]
fn applied_advances_only_once_the_work_it_claims_is_settled() {
    let (store, pool, job) = store_with_job("receive-applied");
    begin(&store, &job, "scope-1");
    store
        .promote_remote_graph(
            job.id,
            graph(vec![photo("a1", "gen-1", "camera/one.jpg")]),
            "zone.target",
        )
        .unwrap();
    assert!(store.advance_applied_token(job.id).is_err());

    // C1.4 settles these; for now the test settles them directly to prove the
    // gate opens exactly when the work is gone, not at some later moment.
    pool.get()
        .unwrap()
        .execute(
            "UPDATE sync_native_resources SET state='verified' WHERE job_id=?1",
            [job.id],
        )
        .unwrap();
    assert_eq!(store.unresolved_resource_count(job.id).unwrap(), 0);
    assert!(store.advance_applied_token(job.id).unwrap());

    let progress = store.native_progress(job.id).unwrap().unwrap();
    assert_eq!(progress.applied_token.as_deref(), Some("zone.target"));
}

#[test]
fn an_empty_zone_promotes_cleanly_and_applies_immediately() {
    let (store, _pool, job) = store_with_job("receive-empty");
    begin(&store, &job, "scope-1");
    let (_, _, resources, applied) = store
        .promote_remote_graph(job.id, RemoteGraph::default(), "zone.target")
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
        .promote_remote_graph(job.id, RemoteGraph::default(), "zone.target")
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
        .promote_remote_graph(job.id, graph, "zone.target")
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
        )
        .unwrap();
    pool.get()
        .unwrap()
        .execute(
            "INSERT INTO sync_entries(job_id,relative_path,native_dirty,native_photo_id)
             VALUES(?1,'a.jpg',2,'photo-1')",
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
