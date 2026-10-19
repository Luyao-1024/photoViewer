//! Dirty truth: what a user's edit creates, and what may later clear it.
//!
//! The rule under test throughout is that a category is cleared only by proof
//! that *that* category was accepted at *that* version. Everything else here —
//! the mid-flight edit, the confirmed create that must not become a second
//! create — is a consequence of that one rule.

use super::*;
use crate::core::sync::store::tests::{new_job, temp_root};
use crate::core::sync::SyncJob;

fn stamp(at: i64, event: &str) -> EditEventStamp {
    EditEventStamp {
        edited_at_ms: at,
        source: "device-a".into(),
        event_id: event.into(),
    }
}

struct Fixture {
    store: SyncStore,
    pool: crate::core::db::DbPool,
    job: SyncJob,
    media_id: i64,
}

fn fixture(name: &str) -> Fixture {
    let pool = crate::core::db::init_pool(&temp_root(name).join("library.db")).unwrap();
    let store = SyncStore::new(pool.clone());
    let root = temp_root(&format!("{name}-media"));
    let job = store.create_job(&new_job(root.clone())).unwrap();
    let root = root.display().to_string();

    // A real media row under the job root, so the covering query has something
    // to match rather than being satisfied by a fabricated entry.
    pool.get()
        .unwrap()
        .execute_batch(&format!(
            "INSERT INTO media_items
                (uri, path, folder_path, mime_type, media_kind, file_mtime, file_size,
                 blake3_hash, indexed_at)
             VALUES('file://{root}/a.jpg', '{root}/a.jpg', '{root}', 'image/jpeg', 'image',
                     1700000000, 1024, 'hash', 1700000000)"
        ))
        .unwrap();
    let media_id = pool
        .get()
        .unwrap()
        .query_row(
            "SELECT id FROM media_items WHERE path = ?1",
            [format!("{root}/a.jpg")],
            |row| row.get(0),
        )
        .unwrap();
    Fixture {
        store,
        pool,
        job,
        media_id,
    }
}

/// Enroll the entry as if it already existed remotely, which is the state a
/// metadata-only update needs.
fn confirm_identity(fixture: &Fixture) -> i64 {
    fixture
        .pool
        .get()
        .unwrap()
        .execute(
            "INSERT INTO sync_entries(job_id, media_id, relative_path, native_photo_id,
                                      native_master_id, native_asset_revision,
                                      native_master_revision)
             VALUES(?1, ?2, 'a.jpg', 'photo-1', 'master-1', 'rev-1', 'mrev-1')",
            rusqlite::params![fixture.job.id, fixture.media_id],
        )
        .unwrap();
    fixture
        .pool
        .get()
        .unwrap()
        .query_row(
            "SELECT id FROM sync_entries WHERE job_id = ?1",
            [fixture.job.id],
            |row| row.get::<_, i64>(0),
        )
        .unwrap()
}

fn note(fixture: &Fixture, category: ChangeCategory, at: i64, event: &str) -> Option<DirtyOutcome> {
    let mut conn = fixture.pool.get().unwrap();
    let tx = conn.transaction().unwrap();
    let outcome = note_local_change_in(&tx, fixture.media_id, category, &stamp(at, event)).unwrap();
    tx.commit().unwrap();
    outcome
}

fn entry_state(fixture: &Fixture, entry_id: i64) -> (NativeDirty, i64, i64, i64) {
    fixture
        .pool
        .get()
        .unwrap()
        .query_row(
            "SELECT native_dirty, native_local_version, native_metadata_version,
                    native_acked_metadata_version
             FROM sync_entries WHERE id = ?1",
            [entry_id],
            |row| {
                let dirty: i64 = row.get(0)?;
                Ok((
                    NativeDirty(dirty as u8),
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                ))
            },
        )
        .unwrap()
}

#[test]
fn a_change_outside_any_job_root_creates_no_obligation() {
    let fixture = fixture("dirty-uncovered");
    fixture
        .pool
        .get()
        .unwrap()
        .execute_batch(
            "INSERT INTO media_items
                (uri, path, folder_path, mime_type, media_kind, file_mtime, file_size,
                 blake3_hash, indexed_at)
             VALUES('file:///elsewhere/b.jpg', '/elsewhere/b.jpg', '/elsewhere', 'image/jpeg',
                     'image', 1700000000, 1024, 'hash', 1700000000)",
        )
        .unwrap();
    let outside = fixture
        .pool
        .get()
        .unwrap()
        .query_row(
            "SELECT id FROM media_items WHERE path = '/elsewhere/b.jpg'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap();

    let mut conn = fixture.pool.get().unwrap();
    let tx = conn.transaction().unwrap();
    let outcome =
        note_local_change_in(&tx, outside, ChangeCategory::Metadata, &stamp(1, "e1")).unwrap();
    tx.commit().unwrap();
    assert!(
        outcome.is_none(),
        "a photo nobody syncs must not gain an obligation — and, since this is the \
         branch that arms the debounce trigger, must not schedule a run either"
    );
    let entries: i64 = fixture
        .pool
        .get()
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM sync_entries WHERE job_id = ?1",
            [fixture.job.id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(entries, 0);
}

#[test]
fn a_change_inside_a_job_root_enrolls_the_photo_as_a_new_obligation() {
    let fixture = fixture("dirty-enroll");
    note(&fixture, ChangeCategory::Metadata, 100, "e1");

    let (dirty, local, _metadata, _acked) = {
        let entry_id = fixture
            .pool
            .get()
            .unwrap()
            .query_row(
                "SELECT id FROM sync_entries WHERE job_id = ?1",
                [fixture.job.id],
                |row| row.get::<_, i64>(0),
            )
            .unwrap();
        entry_state(&fixture, entry_id)
    };
    // Enrolling is an explicit obligation, not a silent success.
    assert_eq!(dirty.bits(), NativeDirty::NEW);
    let entry = fixture.store.uplink_view(fixture.job.id).unwrap().remove(0);
    assert_eq!(entry.metadata_version, 1);
    assert!(entry.has_metadata_change());
    assert_eq!(local, 1);
}

#[test]
fn a_local_change_advances_the_local_and_category_versions_and_stamps_the_event() {
    let fixture = fixture("dirty-versions");
    let entry_id = confirm_identity(&fixture);

    note(
        &fixture,
        ChangeCategory::Metadata,
        1_700_000_000_000,
        "meta-1",
    );
    let (dirty, local, metadata, acked) = entry_state(&fixture, entry_id);
    assert!(dirty.contains(NativeDirty::METADATA));
    assert_eq!((local, metadata), (1, 1));
    // The acknowledged counter never moves on a local edit: it is evidence from
    // the cloud, and there has been no cloud yet.
    assert_eq!(acked, 0);

    let event = super::super::records::read_events(&fixture.pool.get().unwrap(), entry_id)
        .unwrap()
        .metadata
        .unwrap();
    assert_eq!(event.edited_at_ms, 1_700_000_000_000);
    assert_eq!(event.source, "device-a");
    assert_eq!(event.event_id, "meta-1");

    // A second edit of the same category is a distinct version, not a repeat.
    note(
        &fixture,
        ChangeCategory::Metadata,
        1_700_000_005_000,
        "meta-2",
    );
    let (_, local, metadata, _) = entry_state(&fixture, entry_id);
    assert_eq!((local, metadata), (2, 2));
}

#[test]
fn each_category_tracks_its_own_version() {
    let fixture = fixture("dirty-categories");
    let entry_id = confirm_identity(&fixture);
    note(&fixture, ChangeCategory::Metadata, 1, "m");
    note(&fixture, ChangeCategory::Content, 2, "c");

    let entry = &fixture.store.uplink_view(fixture.job.id).unwrap()[0];
    assert_eq!(entry.metadata_version, 1);
    assert_eq!(entry.content_version, 1);
    // One category's edit never advances the other's version, so a content
    // change cannot be cleared by a metadata confirmation.
    assert_eq!(entry.local_version, 2);
    assert!(entry.metadata_event.is_some());
    assert!(entry.content_event.is_some());
    assert!(entry.deletion_event.is_none());
    assert_eq!(entry_id, entry.entry_id);
}

#[test]
fn a_confirmation_at_the_captured_version_clears_only_that_category() {
    let fixture = fixture("dirty-clear");
    let entry_id = confirm_identity(&fixture);
    note(&fixture, ChangeCategory::Metadata, 1, "m");
    note(&fixture, ChangeCategory::Content, 1, "c");

    let outcome = fixture
        .store
        .confirm_category(
            entry_id,
            ChangeCategory::Metadata,
            1,
            ConfirmedIdentity::default(),
        )
        .unwrap();
    assert_eq!(outcome, CategoryOutcome::Cleared);

    let (dirty, _local, metadata, acked) = entry_state(&fixture, entry_id);
    assert_eq!(dirty.bits(), NativeDirty::MODIFIED);
    let remaining = fixture.store.uplink_view(fixture.job.id).unwrap();
    assert!(!remaining[0].has_metadata_change());
    assert!(
        remaining[0].has_content_change(),
        "a metadata confirmation must not clear an outstanding content change"
    );
    assert_eq!((metadata, acked), (1, 1));
}

#[test]
fn an_edit_made_while_the_operation_was_in_flight_survives_its_callback() {
    let fixture = fixture("dirty-race");
    let entry_id = confirm_identity(&fixture);
    note(&fixture, ChangeCategory::Metadata, 1, "first");

    // The user edits again while the first upload is still in flight.
    note(&fixture, ChangeCategory::Metadata, 2, "second");

    // The callback for the first upload carries version 1, which no longer
    // matches. Clearing here would silently drop the user's second edit.
    let outcome = fixture
        .store
        .confirm_category(
            entry_id,
            ChangeCategory::Metadata,
            1,
            ConfirmedIdentity::default(),
        )
        .unwrap();
    assert_eq!(outcome, CategoryOutcome::Superseded);

    let (dirty, local, metadata, acked) = entry_state(&fixture, entry_id);
    assert!(
        dirty.contains(NativeDirty::METADATA),
        "the newer edit stays dirty"
    );
    assert_eq!(local, 2);
    assert_eq!(metadata, 2);
    // The first upload did land, so the acknowledgement still advances: the
    // cloud knows about version 1 even though version 2 is outstanding.
    assert_eq!(acked, 1);
}

#[test]
fn a_confirmed_create_registers_identity_even_while_later_edits_remain_dirty() {
    let fixture = fixture("dirty-create-identity");
    let entry_id = confirm_identity(&fixture);
    note(&fixture, ChangeCategory::Metadata, 1, "m");
    // The captured version is stale because of a newer edit, so the category
    // stays dirty — but the identity still has to be registered.
    note(&fixture, ChangeCategory::Metadata, 2, "m2");

    fixture
        .store
        .confirm_category(
            entry_id,
            ChangeCategory::Metadata,
            1,
            ConfirmedIdentity {
                photo_id: Some("photo-9".into()),
                master_id: Some("master-9".into()),
                asset_revision: Some("rev-9".into()),
                master_revision: Some("mrev-9".into()),
                content_generation: Some("gen-9".into()),
            },
        )
        .unwrap();

    let (photo, master, revision, generation): (String, String, String, String) = fixture
        .pool
        .get()
        .unwrap()
        .query_row(
            "SELECT native_photo_id, native_master_id, native_asset_revision,
                    native_content_generation FROM sync_entries WHERE id = ?1",
            [entry_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert_eq!(photo, "photo-9");
    assert_eq!(master, "master-9");
    assert_eq!(revision, "rev-9");
    assert_eq!(generation, "gen-9");

    // The residual work is now an update against a known record, not another
    // create.
    let (dirty, ..) = entry_state(&fixture, entry_id);
    assert!(dirty.contains(NativeDirty::METADATA));
    assert!(!dirty.contains(NativeDirty::NEW));
}

#[test]
fn the_uplink_view_derives_from_the_rows_and_skips_clean_entries() {
    let fixture = fixture("dirty-view");
    let entry_id = confirm_identity(&fixture);
    assert!(
        fixture
            .store
            .uplink_view(fixture.job.id)
            .unwrap()
            .is_empty(),
        "a clean entry is not work"
    );

    note(&fixture, ChangeCategory::Metadata, 1, "m");
    let view = fixture.store.uplink_view(fixture.job.id).unwrap();
    assert_eq!(view.len(), 1);
    let entry = &view[0];
    assert_eq!(entry.entry_id, entry_id);
    assert_eq!(entry.relative_path, "a.jpg");
    assert_eq!(entry.remote_names(), Some(("photo-1", "master-1")));

    fixture
        .store
        .confirm_category(
            entry_id,
            ChangeCategory::Metadata,
            1,
            ConfirmedIdentity::default(),
        )
        .unwrap();
    assert!(
        fixture
            .store
            .uplink_view(fixture.job.id)
            .unwrap()
            .is_empty(),
        "a confirmed entry is no longer work"
    );
}

#[test]
fn a_trashed_photo_records_metadata_without_a_delete_intent() {
    let fixture = fixture("dirty-trashed");
    confirm_identity(&fixture);
    fixture
        .pool
        .get()
        .unwrap()
        .execute(
            "UPDATE media_items SET trashed_at = 1700000000 WHERE id = ?1",
            [fixture.media_id],
        )
        .unwrap();
    assert!(note(&fixture, ChangeCategory::Metadata, 1, "m").is_some());
    let entry = fixture.store.uplink_view(fixture.job.id).unwrap().remove(0);
    assert_eq!(entry.dirty.bits(), NativeDirty::MODIFIED);
    assert!(entry.has_metadata_change());
    assert_eq!(entry.deletion_version, 0);
    assert!(entry.deletion_event.is_none());
}

#[test]
fn dirty_states_are_exclusive_and_delete_is_four() {
    assert_eq!(NativeDirty(NativeDirty::NEW).names(), vec!["new"]);
    assert_eq!(NativeDirty(NativeDirty::MODIFIED).names(), vec!["modified"]);
    assert_eq!(NativeDirty(NativeDirty::DELETE).names(), vec!["delete"]);
    assert_eq!(NativeDirty::DELETE, 4);
    assert!(NativeDirty(NativeDirty::CLEAN).is_clean());
}

#[test]
fn off_history_stays_empty_during_local_changes() {
    let fixture = fixture("global-off-local-changes");
    confirm_identity(&fixture);
    fixture.store.begin_reset().unwrap();
    assert!(note(&fixture, ChangeCategory::Metadata, 2, "during-reset").is_none());
    fixture.store.resume_local_changes().unwrap();
    assert!(note(&fixture, ChangeCategory::Metadata, 2, "reset-cannot-resume").is_none());
    let reset = fixture.store.reset_intent().unwrap().unwrap();
    assert!(fixture.store.finish_reset(&reset.token).unwrap());

    // The selected account and its folder configuration survive global Off.
    assert!(fixture.store.job_is_selected(fixture.job.id).unwrap());
    assert!(note(&fixture, ChangeCategory::Metadata, 3, "off-edit").is_none());
    assert!(note(&fixture, ChangeCategory::Deletion, 4, "off-delete").is_none());
    let entries: i64 = fixture
        .pool
        .get()
        .unwrap()
        .query_row(
            "SELECT count(*) FROM sync_entries WHERE job_id=?1",
            [fixture.job.id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        entries, 0,
        "Off must retain configuration without rebuilding history"
    );
}

#[test]
fn off_history_does_not_enroll_deletion_of_an_untracked_path() {
    let fixture = fixture("off-missing-untracked");
    fixture.store.begin_reset().unwrap();
    let reset = fixture.store.reset_intent().unwrap().unwrap();
    assert!(fixture.store.finish_reset(&reset.token).unwrap());
    fixture.store.resume_local_changes().unwrap();
    // Re-enabling can precede startup pruning of files removed while Off.
    // A media index row alone is not evidence of an existing sync obligation.
    assert!(note(&fixture, ChangeCategory::Deletion, 1, "startup-prune").is_none());
    assert!(fixture
        .store
        .uplink_view(fixture.job.id)
        .unwrap()
        .is_empty());

    let entry = confirm_identity(&fixture);
    assert!(note(&fixture, ChangeCategory::Deletion, 2, "active-delete").is_some());
    let pending = fixture.store.uplink_view(fixture.job.id).unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].entry_id, entry);
    assert!(pending[0].dirty.contains(NativeDirty::DELETE));
}
