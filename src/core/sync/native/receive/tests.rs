use super::*;

/// A record as `syncserver@2d9f9a7` emits it, key order and all.
fn live_record() -> &'static str {
    r#"{"recordName":"a1","recordType":"CPLAsset","zoneID":{"zoneName":"PrimarySync","ownerRecordName":"_defaultOwner"},"recordChangeTag":"8f14e45f-ceea-467a-9575-1cba2c3a2c3f","fields":{"relativePath":{"type":"STRING","value":"camera/one.jpg"}},"_syncserver":{}}"#
}

/// A tombstone: `recordType` is explicitly null, `deleted` is true, and there
/// is no `fields` key at all.
fn tombstone() -> &'static str {
    r#"{"recordName":"a1","recordType":null,"zoneID":{"zoneName":"PrimarySync","ownerRecordName":"_defaultOwner"},"recordChangeTag":"c4ca4238-a0b9-43f0-9b4b-8f2c1a2e3d4f","deleted":true,"_syncserver":{}}"#
}

#[test]
fn a_live_record_decodes_with_the_exact_wire_names() {
    let record: NativeRecord = serde_json::from_str(live_record()).unwrap();
    assert_eq!(record.record_name, "a1");
    assert_eq!(record.record_type.as_deref(), Some("CPLAsset"));
    assert_eq!(
        record.record_change_tag.as_deref(),
        Some("8f14e45f-ceea-467a-9575-1cba2c3a2c3f")
    );
    assert!(!record.is_tombstone());
    assert!(record.fields.as_ref().unwrap().contains_key("relativePath"));
    // An absent event object is "unknown", never a synthesized timestamp.
    assert!(record.events.unwrap().metadata.is_none());
}

#[test]
fn a_tombstone_is_identified_without_guessing_from_its_type() {
    let record: NativeRecord = serde_json::from_str(tombstone()).unwrap();
    assert!(record.is_tombstone());
    assert!(record.record_type.is_none());
    assert!(record.fields.is_none());
    // The revision on a delete is a fresh one, so equality is the only
    // meaningful comparison with the prior record.
    assert!(record.record_change_tag.is_some());
}

#[test]
fn the_sdk_record_maps_back_to_the_local_projection_shape() {
    let mut fields = std::collections::BTreeMap::new();
    fields.insert(
        "relativePath".to_string(),
        sync_client::Field::string("camera/one.jpg"),
    );
    let events = sync_client::EditEvents {
        metadata_event: Some(sync_client::EditEvent {
            edited_at_ms: 1_700_000_000_000,
            source: "device".into(),
            event_id: "event-1".into(),
        }),
        ..Default::default()
    };
    let mapped = native_record(sync_client::RemoteRecord {
        record_name: "a1".into(),
        record_type: Some(ASSET_RECORD_TYPE.into()),
        zone_id: Some(sync_client::ZoneId::new("PrimarySync")),
        record_change_tag: Some("tag-1".into()),
        fields,
        events,
        deleted: false,
    });
    assert_eq!(mapped.record_type.as_deref(), Some(ASSET_RECORD_TYPE));
    assert_eq!(mapped.zone_id.unwrap().owner_record_name, "_defaultOwner");
    assert_eq!(
        mapped.fields.unwrap()["relativePath"],
        serde_json::json!({"type": "STRING", "value": "camera/one.jpg"})
    );
    let events = mapped.events.unwrap();
    assert_eq!(events.metadata.unwrap().event_id, "event-1");
    assert!(events.content.is_none());
}

#[test]
fn an_sdk_tombstone_maps_without_fields() {
    let events = sync_client::EditEvents {
        deletion_event: Some(sync_client::EditEvent {
            edited_at_ms: 1_700_000_000_001,
            source: "device".into(),
            event_id: "event-2".into(),
        }),
        ..Default::default()
    };
    let mapped = native_record(sync_client::RemoteRecord {
        record_name: "a1".into(),
        record_type: None,
        zone_id: None,
        record_change_tag: Some("tag-2".into()),
        fields: std::collections::BTreeMap::new(),
        events,
        deleted: true,
    });
    assert!(mapped.is_tombstone());
    assert!(mapped.fields.is_none());
    assert_eq!(mapped.events.unwrap().deletion.unwrap().event_id, "event-2");
}

#[test]
fn a_last_page_omits_the_continuation_marker_entirely() {
    // The key is absent, not empty. Treating a missing key as "" would make a
    // paged read loop forever on the final page.
    let last: QueryResponse =
        serde_json::from_str(r#"{"records":[],"syncToken":"zone.abc"}"#).unwrap();
    assert!(last.continuation_marker.is_none());
    assert!(!last.more_pages());

    let paged: QueryResponse = serde_json::from_str(
        r#"{"records":[],"syncToken":"zone.abc","continuationMarker":"query.def"}"#,
    )
    .unwrap();
    assert!(paged.more_pages());
}

#[test]
fn a_zone_entry_can_carry_an_in_band_error_inside_an_http_200() {
    // The SDK surfaces the same per-zone failure, and the records array is
    // meaningless on it: the code is read before the records are.
    let entry = ZoneChangesEntry {
        zone_id: None,
        records: Vec::new(),
        more_coming: false,
        sync_token: None,
        server_error_code: Some(UNKNOWN_ITEM.into()),
        reason: Some("zone is not configured".into()),
    };
    assert_eq!(entry.server_error_code.as_deref(), Some(UNKNOWN_ITEM));
    assert!(entry.records.is_empty());
    assert!(map_zone_error(UNKNOWN_ITEM, entry.reason.as_deref())
        .to_string()
        .contains("not configured"));
}

#[test]
fn an_expired_token_stays_distinguishable_from_a_bad_request() {
    // They recover differently: an expired position is rebuilt from a new
    // snapshot, a bad request is a client bug. Collapsing them would make the
    // client retry a token the server will never accept.
    let expired = map_zone_error(EXPIRED_SYNC_TOKEN, Some("range"));
    assert!(matches!(expired, SyncError::Unsupported(_)));
    assert!(expired.to_string().contains("EXPIRED_SYNC_TOKEN"));

    let bad_request = map_zone_error("BAD_REQUEST", Some("Invalid JSON"));
    assert!(matches!(bad_request, SyncError::Protocol(_)));
}

#[test]
fn zone_identity_always_names_the_owner_the_service_requires() {
    let zone = NativeZone::new("PrimarySync");
    let id = ZoneId::from(&zone);
    assert_eq!(id.zone_name, "PrimarySync");
    assert_eq!(id.owner_record_name, "_defaultOwner");
    // Round-tripping must not lose the owner: the service rejects any other
    // value outright, so a dropped field is a hard 400.
    assert_eq!(id.zone(), zone);
    // The SDK zone is built from the zone name only; the SDK supplies the
    // owner its contract requires.
    assert_eq!(sdk_zone(&zone), sync_client::ZoneId::new("PrimarySync"));
}

#[test]
fn an_omitted_owner_defaults_rather_than_failing_to_parse() {
    let id: ZoneId = serde_json::from_str(r#"{"zoneName":"PrimarySync"}"#).unwrap();
    assert_eq!(id.owner_record_name, "_defaultOwner");
}

#[test]
fn a_receive_needs_every_required_capability_before_it_is_attempted() {
    let mut bits =
        crate::core::sync::native::protocol::demo_pinned_scope("https://s/", &["Z"]).capabilities;
    assert!(receive_is_permitted(&bits));
    bits.incremental_changes = false;
    assert!(!receive_is_permitted(&bits));
    bits.incremental_changes = true;
    bits.snapshots = false;
    assert!(!receive_is_permitted(&bits));
}

#[test]
fn receive_pages_request_100_records_or_the_smaller_server_limit() {
    let endpoint =
        super::super::protocol::NativeEndpoint::parse("https://sync.example.invalid/").unwrap();
    let client = NativeClient::new(&endpoint, &"a".repeat(32)).unwrap();
    let mut pinned = super::super::protocol::demo_pinned_scope(
        "https://sync.example.invalid/",
        &["PrimarySync"],
    );
    for (server_limit, expected) in [(200, 100), (100, 100), (40, 40), (1, 1)] {
        pinned.sync_limits.max_page = server_limit;
        assert_eq!(client.page_limit(&pinned), expected);
    }
}
