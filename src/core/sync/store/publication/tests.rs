//! The admission gate: a photo becomes visible only when its pair is usable.
//!
//! Everything asserted here is about what must **not** happen early. A
//! half-published pair is the failure this stage exists to prevent, so the tests
//! are mostly about refusals, and about restartability rather than happy paths.

use super::*;
use crate::core::sync::store::tests::{new_job, temp_root};
use crate::core::sync::SyncJob;

const GENERATION: &str = "501855bb-18d3-4008-abc3-37e4948bf6f0";

struct Fixture {
    store: SyncStore,
    job: SyncJob,
}

fn fixture(name: &str) -> Fixture {
    let pool = crate::core::db::init_pool(&temp_root(name).join("library.db")).unwrap();
    let store = SyncStore::new(pool);
    let job = store
        .create_job(&new_job(temp_root(&format!("{name}-media"))))
        .unwrap();
    // A receive position, so resource rows have a generation to belong to.
    store
        .begin_native_receive(
            job.id,
            crate::core::sync::native::demo_pinned_scope("https://s.invalid/", &["PrimarySync"])
                .namespace,
            GENERATION,
            "scope-1",
            crate::core::sync::NativeZone::new("PrimarySync"),
        )
        .unwrap();
    Fixture { store, job }
}

/// A projection naming one photo's two resources, which is what makes them owed.
fn owe_pair(fixture: &Fixture, photo: &str, original: &str, thumbnail: &str) {
    let graph = crate::core::sync::RemoteGraph {
        photos: vec![remote_photo(photo, original, thumbnail)],
        records: Vec::new(),
        tombstones: Vec::new(),
    };
    fixture
        .store
        .promote_remote_graph(fixture.job.id, graph, "zone.target")
        .unwrap();
}

fn remote_photo(
    photo_id: &str,
    original_asset: &str,
    thumbnail_asset: &str,
) -> crate::core::sync::RemotePhoto {
    crate::core::sync::RemotePhoto {
        photo_id: photo_id.into(),
        master_id: format!("master-{photo_id}"),
        content_generation: "gen-1".into(),
        relative_path: format!("camera/{photo_id}.jpg"),
        is_favorite: false,
        asset_date_ms: None,
        is_hidden: false,
        original: descriptor(original_asset, 2048),
        thumbnail: descriptor(thumbnail_asset, 512),
        thumbnail_source_sha256: "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
            .into(),
        events: Default::default(),
    }
}

fn descriptor(asset_id: &str, size: u64) -> crate::core::sync::AssetDescriptor {
    crate::core::sync::AssetDescriptor {
        asset_id: asset_id.into(),
        size,
        file_checksum: "N4kXJS+f44FaFFqy99mQC2jGiVb2+sBo/cnZh7fnpBk=".into(),
        mime_type: "image/jpeg".into(),
        width: Some(800),
        height: Some(600),
    }
}

#[test]
fn a_projected_pair_creates_one_obligation_per_role() {
    let fixture = fixture("pub-owe-pair");
    owe_pair(&fixture, "a1", "asset-original", "asset-thumb");

    let outstanding = fixture.store.outstanding_resources(fixture.job.id).unwrap();
    assert_eq!(outstanding.len(), 2);
    // The original is attempted first: the thumbnail is generated from it, and
    // publishing a thumbnail with no original would leave a tile nothing else
    // agrees with.
    assert_eq!(outstanding[0].role, "original");
    assert_eq!(outstanding[1].role, "thumbnail");
    for work in &outstanding {
        assert_eq!(work.state, ResourceState::Pending);
        assert_eq!(work.record_name.as_deref(), Some("a1"));
        assert_eq!(work.generation, GENERATION);
    }
}

#[test]
fn a_verified_role_leaves_the_outstanding_set() {
    let fixture = fixture("pub-verify-one");
    owe_pair(&fixture, "a1", "asset-original", "asset-thumb");

    fixture
        .store
        .stage_resource(
            fixture.job.id,
            "asset-original",
            "original",
            "/staging/o.part",
        )
        .unwrap();
    let outstanding = fixture.store.outstanding_resources(fixture.job.id).unwrap();
    assert_eq!(
        outstanding.len(),
        2,
        "a staged role is still owed until verified"
    );

    fixture
        .store
        .verify_resource(
            fixture.job.id,
            "asset-original",
            "original",
            "/cache/thumb-v4/x.jpg",
        )
        .unwrap();
    let outstanding = fixture.store.outstanding_resources(fixture.job.id).unwrap();
    assert_eq!(outstanding.len(), 1);
    assert_eq!(outstanding[0].role, "thumbnail");
}

#[test]
fn a_publication_cannot_complete_until_both_roles_are_verified() {
    let fixture = fixture("pub-gate");
    owe_pair(&fixture, "a1", "asset-original", "asset-thumb");
    fixture
        .store
        .begin_publication(
            fixture.job.id,
            None,
            "pub-a1",
            "a1",
            Some("master-a1"),
            Some("asset-original"),
            Some("asset-thumb"),
        )
        .unwrap();

    // Nothing verified: the photo is not usable.
    assert!(!fixture.store.complete_publication("pub-a1").unwrap());
    // One role verified: still not. This is the half-pair case.
    fixture
        .store
        .verify_resource(fixture.job.id, "asset-original", "original", "/cache/o.jpg")
        .unwrap();
    assert!(
        !fixture.store.complete_publication("pub-a1").unwrap(),
        "a verified original with no thumbnail must not publish"
    );
    // Both: now, and only now.
    fixture
        .store
        .verify_resource(fixture.job.id, "asset-thumb", "thumbnail", "/cache/t.jpg")
        .unwrap();
    assert!(fixture.store.complete_publication("pub-a1").unwrap());
    assert!(fixture
        .store
        .pending_publications(fixture.job.id)
        .unwrap()
        .is_empty());
}

#[test]
fn a_failed_role_is_not_retryable_as_if_it_were_new() {
    let fixture = fixture("pub-failed");
    owe_pair(&fixture, "a1", "asset-original", "asset-thumb");
    fixture
        .store
        .fail_resource(fixture.job.id, "asset-original", "original", "timeout")
        .unwrap();

    // A failed role drops out of the automatic retry set: retrying it forever
    // against a promise the service has withdrawn is how a cycle spins.
    let outstanding = fixture.store.outstanding_resources(fixture.job.id).unwrap();
    assert_eq!(outstanding.len(), 1);
    assert_eq!(outstanding[0].role, "thumbnail");

    // And it can never satisfy the gate on its own.
    fixture
        .store
        .begin_publication(
            fixture.job.id,
            None,
            "pub-failed",
            "a1",
            None,
            Some("asset-original"),
            Some("asset-thumb"),
        )
        .unwrap();
    fixture
        .store
        .verify_resource(fixture.job.id, "asset-thumb", "thumbnail", "/cache/t.jpg")
        .unwrap();
    assert!(!fixture.store.complete_publication("pub-failed").unwrap());
}

#[test]
fn reopening_a_publication_after_a_restart_does_not_duplicate_it() {
    let fixture = fixture("pub-restart");
    owe_pair(&fixture, "a1", "asset-original", "asset-thumb");
    for _ in 0..3 {
        fixture
            .store
            .begin_publication(
                fixture.job.id,
                None,
                "pub-restart",
                "a1",
                Some("master-a1"),
                Some("asset-original"),
                Some("asset-thumb"),
            )
            .unwrap();
    }
    let intents = fixture.store.pending_publications(fixture.job.id).unwrap();
    assert_eq!(
        intents.len(),
        1,
        "a retry must reuse the intent, not add one"
    );
    assert_eq!(intents[0].publication_id, "pub-restart");
}

#[test]
fn a_verified_resource_is_not_lost_when_the_projection_is_replaced() {
    let fixture = fixture("pub-replace");
    owe_pair(&fixture, "a1", "asset-original", "asset-thumb");
    fixture
        .store
        .verify_resource(fixture.job.id, "asset-original", "original", "/cache/o.jpg")
        .unwrap();

    // A newer round re-promotes the same photo. The projection is replaced
    // wholesale, but the obligation it restates for the same asset keeps its
    // verified state — the bytes are still on disk and still match.
    owe_pair(&fixture, "a1", "asset-original", "asset-thumb");
    let outstanding = fixture.store.outstanding_resources(fixture.job.id).unwrap();
    assert_eq!(outstanding.len(), 1);
    assert_eq!(outstanding[0].role, "thumbnail");
}

#[test]
fn changing_the_generation_owes_the_pair_again() {
    let fixture = fixture("pub-generation");
    owe_pair(&fixture, "a1", "asset-original", "asset-thumb");
    fixture
        .store
        .verify_resource(fixture.job.id, "asset-original", "original", "/cache/o.jpg")
        .unwrap();
    fixture
        .store
        .verify_resource(fixture.job.id, "asset-thumb", "thumbnail", "/cache/t.jpg")
        .unwrap();

    // A rebuilt server means every previous "verified" answer was about bytes
    // this client no longer has a promise for.
    fixture
        .store
        .begin_native_receive(
            fixture.job.id,
            crate::core::sync::native::demo_pinned_scope("https://s.invalid/", &["PrimarySync"])
                .namespace,
            "11111111-2222-4333-8444-555555555555",
            "scope-1",
            crate::core::sync::NativeZone::new("PrimarySync"),
        )
        .unwrap();
    owe_pair(&fixture, "a1", "asset-original", "asset-thumb");
    let outstanding = fixture.store.outstanding_resources(fixture.job.id).unwrap();
    assert_eq!(
        outstanding.len(),
        2,
        "a new generation owes the whole pair again"
    );
}

#[test]
fn an_operation_on_a_resource_that_was_never_owed_is_refused() {
    let fixture = fixture("pub-unowed");
    let error = fixture
        .store
        .stage_resource(fixture.job.id, "asset-unknown", "original", "/staging/o")
        .unwrap_err();
    assert!(
        error.to_string().contains("no resource obligation"),
        "{error}"
    );
}

#[test]
fn one_complete_staged_pair_does_not_wait_for_another_photo() {
    let fixture = fixture("pub-per-photo-barrier");
    fixture
        .store
        .promote_remote_graph(
            fixture.job.id,
            crate::core::sync::RemoteGraph {
                photos: vec![
                    remote_photo("first", "first-original", "first-thumb"),
                    remote_photo("second", "second-original", "second-thumb"),
                ],
                records: Vec::new(),
                tombstones: Vec::new(),
            },
            "zone.target",
        )
        .unwrap();
    for (asset, role) in [("first-original", "original"), ("first-thumb", "thumbnail")] {
        fixture
            .store
            .stage_resource(fixture.job.id, asset, role, "/owned/staged-role")
            .unwrap();
    }
    let intents = fixture.store.pending_publications(fixture.job.id).unwrap();
    let first = intents
        .iter()
        .find(|intent| intent.photo_id == "first")
        .unwrap();
    let second = intents
        .iter()
        .find(|intent| intent.photo_id == "second")
        .unwrap();
    assert!(fixture
        .store
        .publication_ready(&first.publication_id)
        .unwrap());
    assert!(!fixture
        .store
        .publication_ready(&second.publication_id)
        .unwrap());
    assert!(fixture
        .store
        .native_progress(fixture.job.id)
        .unwrap()
        .unwrap()
        .applied_token
        .is_none());
}
