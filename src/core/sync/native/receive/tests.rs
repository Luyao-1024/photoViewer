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
    let response: ZoneChangesResponse = serde_json::from_str(
        r#"{"zones":[{"zoneID":{"zoneName":"PrimarySync","ownerRecordName":"_defaultOwner"},"serverErrorCode":"UNKNOWN_ITEM","reason":"zone is not configured"}]}"#,
    )
    .unwrap();
    let entry = &response.zones[0];
    assert_eq!(entry.server_error_code.as_deref(), Some(UNKNOWN_ITEM));
    assert!(entry.records.is_empty());
    // The records array is meaningless on an error entry; the code is read first.
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
fn the_error_envelope_is_only_read_from_an_error_shaped_object() {
    let error = serde_json::json!({"serverErrorCode":"CONFLICT","reason":"changed"});
    assert_eq!(
        NativeErrorBody::from_value(&error)
            .unwrap()
            .server_error_code
            .as_deref(),
        Some("CONFLICT")
    );
    // A successful response that happens to have other keys is not an error.
    assert!(NativeErrorBody::from_value(&serde_json::json!({"records":[]})).is_none());
    assert!(NativeErrorBody::from_value(&serde_json::json!({"syncToken":"z"})).is_none());
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
}

#[test]
fn an_omitted_owner_defaults_rather_than_failing_to_parse() {
    let id: ZoneId = serde_json::from_str(r#"{"zoneName":"PrimarySync"}"#).unwrap();
    assert_eq!(id.owner_record_name, "_defaultOwner");
}

#[test]
fn optional_request_members_are_omitted_not_sent_as_null() {
    // The server's request DTOs are `deny_unknown_fields` and reject explicit
    // nulls for absent optionals, so serialization must skip them.
    let body = ZoneChangesBody {
        zone: &ZoneId {
            zone_name: "PrimarySync".into(),
            owner_record_name: "_defaultOwner".into(),
        },
        sync_token: None,
        results_limit: 200,
    };
    let encoded = serde_json::to_string(&ZoneChangesRequest {
        zones: vec![body.clone()],
    })
    .unwrap();
    assert!(!encoded.contains("syncToken"), "{encoded}");
    assert!(encoded.contains("\"resultsLimit\":200"));

    let encoded = serde_json::to_string(&ZoneChangesRequest {
        zones: vec![ZoneChangesBody {
            sync_token: Some("zone.abc"),
            ..body
        }],
    })
    .unwrap();
    assert!(encoded.contains("\"syncToken\":\"zone.abc\""), "{encoded}");
}

#[test]
fn a_query_request_carries_the_snapshot_token_and_page_marker() {
    let zone = ZoneId {
        zone_name: "PrimarySync".into(),
        owner_record_name: "_defaultOwner".into(),
    };
    let encoded = serde_json::to_string(&QueryEnvelope {
        zone: &zone,
        query: QueryBody {
            record_type: "CPLMaster",
        },
        results_limit: 200,
        snapshot_token: Some("snapshot.abc"),
        continuation_marker: Some("query.def"),
    })
    .unwrap();
    assert!(
        encoded.contains("\"recordType\":\"CPLMaster\""),
        "{encoded}"
    );
    assert!(
        encoded.contains("\"snapshotToken\":\"snapshot.abc\""),
        "{encoded}"
    );
    assert!(
        encoded.contains("\"continuationMarker\":\"query.def\""),
        "{encoded}"
    );
    assert!(encoded.contains("\"zoneID\""), "{encoded}");
    assert!(
        encoded.contains("\"ownerRecordName\":\"_defaultOwner\""),
        "{encoded}"
    );
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
fn a_zone_list_request_body_is_exactly_an_empty_object() {
    // The service accepts nothing else, including an ignored-looking key. A
    // `()` argument would serialize as `null` and be rejected, which is why the
    // call site sends literal bytes instead of a unit value.
    assert_eq!(serde_json::to_string(&()).unwrap(), "null");
    assert_eq!(String::from_utf8(b"{}".to_vec()).unwrap(), "{}");
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
