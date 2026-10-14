use super::*;
use crate::core::sync::native::receive::{ASSET_RECORD_TYPE, MASTER_RECORD_TYPE};
use serde_json::json;

fn zone() -> ZoneId {
    ZoneId {
        zone_name: "PrimarySync".into(),
        owner_record_name: "_defaultOwner".into(),
    }
}

fn field(kind: &str, value: serde_json::Value) -> serde_json::Value {
    json!({ "type": kind, "value": value })
}

fn asset_descriptor(asset_id: &str, size: u64) -> serde_json::Value {
    json!({
        "assetId": asset_id,
        "size": size,
        "fileChecksum": "47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU=",
        "checksumAlgorithm": "SHA256",
        "mimeType": "image/jpeg",
        "width": 800,
        "height": 600
    })
}

fn master(name: &str, photo_id: &str, generation: &str) -> NativeRecord {
    NativeRecord {
        record_name: name.into(),
        record_type: Some(MASTER_RECORD_TYPE.into()),
        zone_id: Some(zone()),
        record_change_tag: Some(format!("tag-{name}")),
        fields: Some(
            json!({
                "photoId": field("STRING", photo_id.into()),
                "contentGeneration": field("STRING", generation.into()),
                "resOriginalRes": field("ASSETID", asset_descriptor("asset-original", 2048)),
                "resJPEGThumbRes": field("ASSETID", asset_descriptor("asset-thumb", 512)),
            })
            .as_object()
            .unwrap()
            .clone(),
        ),
        events: None,
        deleted: false,
    }
}

fn asset(name: &str, master_name: &str, generation: &str, path: &str) -> NativeRecord {
    NativeRecord {
        record_name: name.into(),
        record_type: Some(ASSET_RECORD_TYPE.into()),
        zone_id: Some(zone()),
        record_change_tag: Some(format!("tag-{name}")),
        fields: Some(
            json!({
                "masterRef": field(
                    "REFERENCE",
                    json!({ "recordName": master_name, "action": "NONE" })
                ),
                "contentGeneration": field("STRING", generation.into()),
                "relativePath": field("STRING", path.into()),
                "isFavorite": field("INT64", json!(1)),
                "assetDate": field("TIMESTAMP", json!(1_700_000_000_000i64)),
                "isHidden": field("INT64", json!(0)),
                "thumbnailSourceOriginalSHA256": field(
                    "STRING",
                    "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855".into()
                ),
            })
            .as_object()
            .unwrap()
            .clone(),
        ),
        events: None,
        deleted: false,
    }
}

fn set_of(records: Vec<NativeRecord>) -> RecordSet {
    let mut set = RecordSet::default();
    for record in records {
        set.apply(record);
    }
    set
}

#[test]
fn a_complete_pair_projects_a_typed_photo() {
    let set = set_of(vec![
        master("m1", "a1", "gen-1"),
        asset("a1", "m1", "gen-1", "camera/one.jpg"),
    ]);
    let graph = set.to_graph(&zone()).unwrap();

    assert_eq!(graph.photo_count(), 1);
    let photo = &graph.photos[0];
    assert_eq!(photo.photo_id, "a1");
    assert_eq!(photo.master_id, "m1");
    assert_eq!(photo.relative_path, "camera/one.jpg");
    assert!(photo.is_favorite);
    assert_eq!(photo.asset_date_ms, Some(1_700_000_000_000));
    assert!(!photo.is_hidden);
    assert_eq!(photo.original.asset_id, "asset-original");
    assert_eq!(photo.original.size, 2048);
    assert_eq!(photo.thumbnail.size, 512);
    // Two roles per photo, original first — the thumbnail is generated from it.
    let resources = photo.resources();
    assert_eq!(resources.len(), 2);
    assert_eq!(resources[0].role, "original");
    assert_eq!(resources[1].role, "thumbnail");
    assert_eq!(resources[0].record_name, "a1");
    assert!(graph.unknown_record_types().is_empty());
}

#[test]
fn a_signed_download_url_is_never_becomes_part_of_the_identity() {
    // The service mints a fresh downloadURL and receipt on every read. If
    // either reached the projection, a restart would compare two different URLs
    // for one asset and conclude the content changed.
    let set = set_of(vec![
        master("m1", "a1", "gen-1"),
        asset("a1", "m1", "gen-1", "camera/one.jpg"),
    ]);
    let graph = set.to_graph(&zone()).unwrap();
    let photo = &graph.photos[0];
    let encoded = serde_json::to_string(photo).unwrap();
    assert!(!encoded.contains("downloadURL"), "{encoded}");
    assert!(!encoded.contains("receipt"), "{encoded}");
}

#[test]
fn an_asset_without_its_master_is_rejected() {
    let set = set_of(vec![asset("a1", "m1", "gen-1", "camera/one.jpg")]);
    assert_eq!(
        set.to_graph(&zone()).unwrap_err(),
        GraphError::DanglingMaster {
            name: "a1".into(),
            master: "m1".into(),
        }
    );
}

#[test]
fn a_master_no_asset_references_is_rejected() {
    // Half a pair must never be promoted: an orphan master describes resources
    // this client cannot attribute to any photo.
    let set = set_of(vec![master("m1", "a1", "gen-1")]);
    assert_eq!(
        set.to_graph(&zone()).unwrap_err(),
        GraphError::UnreferencedMaster {
            master: "m1".into()
        }
    );
}

#[test]
fn a_master_naming_a_different_photo_is_rejected() {
    let set = set_of(vec![
        master("m1", "other", "gen-1"),
        asset("a1", "m1", "gen-1", "camera/one.jpg"),
    ]);
    assert_eq!(
        set.to_graph(&zone()).unwrap_err(),
        GraphError::MasterPhotoIdMismatch {
            master: "m1".into(),
            claimed: "other".into(),
            actual: "a1".into(),
        }
    );
}

#[test]
fn mismatched_content_generations_are_rejected() {
    // Two different generations mean the pair's resources and its field
    // baselines describe different versions; accepting either would publish a
    // photo whose thumbnail came from another original.
    let set = set_of(vec![
        master("m1", "a1", "gen-1"),
        asset("a1", "m1", "gen-2", "camera/one.jpg"),
    ]);
    assert_eq!(
        set.to_graph(&zone()).unwrap_err(),
        GraphError::GenerationMismatch {
            name: "a1".into(),
            master: "m1".into(),
        }
    );
}

#[test]
fn a_cross_zone_master_reference_is_rejected() {
    let mut foreign = master("m1", "a1", "gen-1");
    foreign.zone_id = Some(ZoneId {
        zone_name: "OtherZone".into(),
        owner_record_name: "_defaultOwner".into(),
    });
    let set = set_of(vec![foreign, asset("a1", "m1", "gen-1", "camera/one.jpg")]);
    assert!(matches!(
        set.to_graph(&zone()).unwrap_err(),
        GraphError::DanglingMaster { .. }
    ));
}

#[test]
fn unsafe_relative_paths_are_rejected_before_reaching_the_filesystem() {
    for path in [
        "/absolute/one.jpg",
        "../escape.jpg",
        "camera/../../escape.jpg",
        "camera//one.jpg",
        "camera/./one.jpg",
        r"camera\one.jpg",
        "",
    ] {
        let set = set_of(vec![
            master("m1", "a1", "gen-1"),
            asset("a1", "m1", "gen-1", path),
        ]);
        assert!(
            matches!(set.to_graph(&zone()), Err(GraphError::InvalidPath { .. })),
            "{path:?} must be rejected"
        );
    }
}

#[test]
fn two_photos_cannot_claim_the_same_path_in_one_zone() {
    let set = set_of(vec![
        master("m1", "a1", "gen-1"),
        master("m2", "a2", "gen-2"),
        asset("a1", "m1", "gen-1", "camera/one.jpg"),
        asset("a2", "m2", "gen-2", "camera/one.jpg"),
    ]);
    assert!(matches!(
        set.to_graph(&zone()).unwrap_err(),
        GraphError::DuplicatePath { .. }
    ));
}

#[test]
fn a_non_sha256_thumbnail_digest_is_rejected() {
    let mut broken = asset("a1", "m1", "gen-1", "camera/one.jpg");
    broken.fields.as_mut().unwrap().insert(
        "thumbnailSourceOriginalSHA256".into(),
        field("STRING", "not-a-digest".into()),
    );
    let set = set_of(vec![master("m1", "a1", "gen-1"), broken]);
    assert!(matches!(
        set.to_graph(&zone()).unwrap_err(),
        GraphError::InvalidThumbnailDigest { .. }
    ));
}

#[test]
fn a_missing_asset_reference_is_rejected() {
    let mut bare = master("m1", "a1", "gen-1");
    bare.fields.as_mut().unwrap().remove("resOriginalRes");
    let set = set_of(vec![bare, asset("a1", "m1", "gen-1", "camera/one.jpg")]);
    assert_eq!(
        set.to_graph(&zone()).unwrap_err(),
        GraphError::MissingAsset {
            name: "a1".into(),
            field: "resOriginalRes",
        }
    );
}

#[test]
fn unknown_record_types_are_retained_and_never_counted_as_photos() {
    let mut album = asset("a1", "m1", "gen-1", "camera/one.jpg");
    album.record_name = "album-1".into();
    album.record_type = Some("Album".into());
    let set = set_of(vec![
        master("m1", "a1", "gen-1"),
        asset("a1", "m1", "gen-1", "camera/one.jpg"),
        album,
    ]);
    let graph = set.to_graph(&zone()).unwrap();

    assert_eq!(graph.photo_count(), 1);
    assert_eq!(graph.records.len(), 1);
    assert_eq!(graph.records[0].record_type, "Album");
    assert_eq!(graph.unknown_record_types().get("Album"), Some(&1));
}

#[test]
fn a_tombstone_removes_a_record_and_a_later_event_can_re_add_it() {
    let mut set = set_of(vec![
        master("m1", "a1", "gen-1"),
        asset("a1", "m1", "gen-1", "camera/one.jpg"),
    ]);
    assert_eq!(set.to_graph(&zone()).unwrap().photo_count(), 1);

    // A delete arrives as an atomic pair, so both tombstones land together.
    for name in ["a1", "m1"] {
        set.apply(NativeRecord {
            record_name: name.into(),
            record_type: None,
            zone_id: Some(zone()),
            record_change_tag: Some(format!("tag-{name}-delete")),
            fields: None,
            events: None,
            deleted: true,
        });
    }
    assert_eq!(set.to_graph(&zone()).unwrap().photo_count(), 0);

    // A fresh pair at a new name projects cleanly again.
    set.apply(master("m2", "a2", "gen-2"));
    set.apply(asset("a2", "m2", "gen-2", "camera/two.jpg"));
    let graph = set.to_graph(&zone()).unwrap();
    assert_eq!(graph.photo_count(), 1);
    assert_eq!(graph.photos[0].photo_id, "a2");
}

#[test]
fn a_record_without_a_change_tag_cannot_be_projected_as_an_unknown_type() {
    let mut anonymous = asset("a1", "m1", "gen-1", "camera/one.jpg");
    anonymous.record_name = "album-1".into();
    anonymous.record_type = Some("Album".into());
    anonymous.record_change_tag = None;
    let set = set_of(vec![anonymous]);
    assert_eq!(
        set.to_graph(&zone()).unwrap_err(),
        GraphError::MissingRevision {
            name: "album-1".into()
        }
    );
}

#[test]
fn events_survive_into_the_projection_and_unknown_stays_unknown() {
    let mut with_events = asset("a1", "m1", "gen-1", "camera/one.jpg");
    with_events.events = Some(crate::core::sync::native::receive::EditEvents {
        metadata: Some(EditEvent {
            edited_at_ms: 1_700_000_000_000,
            source: "device-a".into(),
            event_id: "meta-1".into(),
        }),
        content: None,
        deletion: None,
    });
    let set = set_of(vec![master("m1", "a1", "gen-1"), with_events]);
    let graph = set.to_graph(&zone()).unwrap();
    let metadata = graph.photos[0].events.metadata.as_ref().unwrap();
    assert_eq!(metadata.source, "device-a");
    // An absent category event stays absent: it must never be synthesized.
    assert!(graph.photos[0].events.content.is_none());
    assert!(graph.photos[0].events.deletion.is_none());
}

#[test]
fn the_latest_event_uses_time_then_a_stable_tie() {
    let early = EditEvent {
        edited_at_ms: 100,
        source: "device-a".into(),
        event_id: "e1".into(),
    };
    let late = EditEvent {
        edited_at_ms: 200,
        source: "device-b".into(),
        event_id: "e2".into(),
    };
    let tie_a = EditEvent {
        edited_at_ms: 200,
        source: "device-a".into(),
        event_id: "e3".into(),
    };
    assert_eq!(latest_event(Some(&early), Some(&late)), Some(late.clone()));
    // Same instant: the tie is broken by a stable key, so two devices holding
    // the same pair of events reach the same answer regardless of the order
    // they learned them in. Which one wins is pinned by the rule, not by
    // arrival — only that it never depends on arrival.
    let forward = latest_event(Some(&late), Some(&tie_a));
    let backward = latest_event(Some(&tie_a), Some(&late));
    assert_eq!(forward, backward);
    assert!(forward.is_some());
    assert_eq!(latest_event(None, Some(&early)), Some(early.clone()));
    assert_eq!(latest_event(Some(&early), None), Some(early.clone()));
    assert_eq!(latest_event(None, None), None);
}
