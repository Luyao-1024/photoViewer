use super::*;

/// Captured verbatim from `GET /v1/capabilities` on `syncserver@2d9f9a7`
/// (stage M4, protocolVersion 1, schemaVersion 5). Pinning the real payload
/// here means a field rename on either side fails a unit test instead of
/// silently weakening the capability gate.
const CAPTABILITIES_M4: &str = r#"{
  "service": "syncserver",
  "version": "0.1.0",
  "stage": "M4",
  "namespace": {
    "tenant": "local",
    "container": "com.apple.photos.cloud",
    "environment": "production",
    "database": "private"
  },
  "zones": ["PrimarySync"],
  "publicBaseURL": "https://sync.example.invalid/",
  "databaseGeneration": "501855bb-18d3-4008-abc3-37e4948bf6f0",
  "schemaVersion": 5,
  "capabilities": {
    "zoneListing": true,
    "recordCrud": true,
    "snapshots": true,
    "incrementalChanges": true,
    "uploadLeases": true,
    "fencing": true,
    "tokenCurrentGate": true,
    "uploadPhases": true,
    "operationStatusRead": true,
    "queryBatch": true,
    "changeAliases": true,
    "assetUpload": true,
    "assetDownload": true,
    "photoPublication": true,
    "appleAuthentication": false,
    "sharing": false,
    "uploadResume": false
  },
  "limits": {
    "maxAssetBytes": 536870912,
    "thumbnailMaxBytes": 2097152,
    "uploadConcurrency": 2,
    "downloadConcurrency": 8,
    "uploadIdleTimeoutSecs": 30,
    "uploadTotalTimeoutSecs": 3600,
    "dbQueueCapacity": 128,
    "jsonBodyLimit": 1048576,
    "leaseTtlSecs": 90,
    "leaseHeartbeatSecs": 30,
    "controlQueueCapacity": 16
  },
  "protocolVersion": 1,
  "checksumAlgorithm": "SHA256",
  "historyRetention": "all",
  "recordFieldTypes": [
    "STRING", "INT64", "DOUBLE", "TIMESTAMP", "BYTES", "REFERENCE",
    "STRING_LIST", "INT64_LIST", "DOUBLE_LIST", "TIMESTAMP_LIST", "BYTES_LIST",
    "REFERENCE_LIST", "ASSETID"
  ],
  "syncLimits": { "maxPage": 200, "maxZones": 20 },
  "recordLimits": {
    "maxOperations": 400,
    "maxRecordBytes": 1048576,
    "maxResultBytes": 8388608
  }
}"#;

fn capabilities() -> NativeCapabilitiesResponse {
    serde_json::from_str(CAPTABILITIES_M4).expect("captured capabilities must decode")
}

fn endpoint() -> NativeEndpoint {
    NativeEndpoint::parse("https://sync.example.invalid/").expect("endpoint must normalize")
}

#[test]
fn captured_capabilities_decode_with_the_expected_wire_names() {
    let decoded = capabilities();
    assert_eq!(decoded.service, "syncserver");
    assert_eq!(decoded.stage, "M4");
    assert_eq!(decoded.protocol_version, SUPPORTED_PROTOCOL_VERSION);
    assert_eq!(decoded.schema_version, 5);
    assert_eq!(decoded.namespace.tenant, "local");
    assert_eq!(decoded.namespace.container, "com.apple.photos.cloud");
    assert_eq!(decoded.namespace.environment, "production");
    assert_eq!(decoded.namespace.database, "private");
    assert_eq!(decoded.zones, vec!["PrimarySync".to_string()]);
    assert_eq!(decoded.sync_limits.max_page, 200);
    assert_eq!(decoded.record_limits.max_operations, 400);
    assert_eq!(decoded.limits.lease_ttl_secs, 90);
    assert_eq!(decoded.limits.lease_heartbeat_secs, 30);
}

#[test]
fn every_required_capability_is_enabled_by_the_captured_server() {
    assert!(capabilities().capabilities.missing_required().is_empty());
    assert!(capabilities()
        .capabilities
        .claimed_optional_features()
        .is_empty());
}

#[test]
fn required_capability_wire_names_match_the_captured_payload() {
    // A rename on the server side must fail here rather than turning a
    // required bit into a silently-unmet requirement.
    for capability in REQUIRED_CAPABILITIES {
        let key = format!("\"{}\": true", capability.wire_name());
        assert!(
            CAPTABILITIES_M4.contains(&key),
            "{} is absent from the captured payload",
            key
        );
    }
    for capability in FORBIDDEN_CAPABILITIES {
        let key = format!("\"{}\": false", capability.wire_name());
        assert!(
            CAPTABILITIES_M4.contains(&key),
            "{} is absent from the captured payload",
            key
        );
    }
}

#[test]
fn unknown_capability_members_do_not_break_decoding() {
    // The server may add bits; refusing to parse would take the client offline.
    let extended = CAPTABILITIES_M4.replace(
        "\"zoneListing\": true",
        "\"zoneListing\": true, \"futureMode\": true",
    );
    let decoded: NativeCapabilitiesResponse = serde_json::from_str(&extended).unwrap();
    assert!(decoded.capabilities.zone_listing);
}

#[test]
fn accepted_capabilities_pin_the_configured_scope() {
    let pinned = validate_capabilities(&endpoint(), &capabilities()).unwrap();
    assert_eq!(pinned.public_base_url, "https://sync.example.invalid/");
    assert_eq!(pinned.namespace.container, "com.apple.photos.cloud");
    assert_eq!(
        pinned.database_generation,
        "501855bb-18d3-4008-abc3-37e4948bf6f0"
    );
    assert_eq!(pinned.protocol_version, 1);
    assert_eq!(pinned.limits.max_asset_bytes, 536870912);
    assert!(pinned.has_zone("PrimarySync"));
    assert!(!pinned.has_zone("OtherSync"));
}

#[test]
fn a_missing_required_capability_names_every_absent_bit() {
    let mut decoded = capabilities();
    decoded.capabilities.photo_publication = false;
    decoded.capabilities.upload_leases = false;
    let failure = validate_capabilities(&endpoint(), &decoded).unwrap_err();
    assert_eq!(
        failure,
        NativeProbeFailure::MissingCapability {
            missing: vec!["uploadLeases", "photoPublication"],
        }
    );
    // The message is what the user sees, so it must carry the bit names.
    assert!(failure.to_string().contains("uploadLeases"));
    assert!(failure.to_string().contains("photoPublication"));
}

#[test]
fn optional_and_forbidden_bits_do_not_enable_or_block_features() {
    // queryBatch and changeAliases only change batch shape.
    let mut decoded = capabilities();
    decoded.capabilities.query_batch = false;
    decoded.capabilities.change_aliases = false;
    assert!(validate_capabilities(&endpoint(), &decoded).is_ok());

    // A future server announcing them changes nothing C1 does.
    let mut decoded = capabilities();
    decoded.capabilities.upload_resume = true;
    decoded.capabilities.sharing = true;
    assert!(validate_capabilities(&endpoint(), &decoded).is_ok());
    assert_eq!(
        decoded.capabilities.claimed_optional_features(),
        vec!["uploadResume", "sharing"]
    );
}

#[test]
fn a_textual_stage_alone_never_enables_features() {
    let mut decoded = capabilities();
    decoded.stage = "M9".into();
    decoded.capabilities.record_crud = false;
    let failure = validate_capabilities(&endpoint(), &decoded).unwrap_err();
    assert!(matches!(
        failure,
        NativeProbeFailure::MissingCapability { .. }
    ));
}

#[test]
fn an_unsupported_protocol_version_is_refused_before_anything_else() {
    let mut decoded = capabilities();
    decoded.protocol_version = 2;
    assert_eq!(
        validate_capabilities(&endpoint(), &decoded).unwrap_err(),
        NativeProbeFailure::ProtocolVersion { found: 2 }
    );
}

#[test]
fn a_foreign_service_on_the_same_path_is_refused() {
    let mut decoded = capabilities();
    decoded.service = "nextcloud".into();
    assert_eq!(
        validate_capabilities(&endpoint(), &decoded).unwrap_err(),
        NativeProbeFailure::UnexpectedService {
            found: "nextcloud".into()
        }
    );
}

#[test]
fn a_different_checksum_algorithm_is_refused() {
    let mut decoded = capabilities();
    decoded.checksum_algorithm = "BLAKE3".into();
    assert_eq!(
        validate_capabilities(&endpoint(), &decoded).unwrap_err(),
        NativeProbeFailure::ChecksumAlgorithm {
            found: "BLAKE3".into()
        }
    );
}

#[test]
fn a_malformed_database_generation_is_refused() {
    for found in ["", "not-a-uuid", "501855BB-18D3-4008-ABC3-37E4948BF6F0"] {
        let mut decoded = capabilities();
        decoded.database_generation = found.into();
        assert_eq!(
            validate_capabilities(&endpoint(), &decoded).unwrap_err(),
            NativeProbeFailure::DatabaseGeneration {
                found: found.into()
            }
        );
    }
}

#[test]
fn a_public_base_url_on_another_origin_is_refused() {
    for announced in [
        "https://other.example.invalid/",
        "http://sync.example.invalid/",
        "https://sync.example.invalid:8443/",
    ] {
        let mut decoded = capabilities();
        decoded.public_base_url = announced.into();
        assert!(
            matches!(
                validate_capabilities(&endpoint(), &decoded).unwrap_err(),
                NativeProbeFailure::PublicBaseUrlMismatch { .. }
            ),
            "{announced} must not be accepted for a different origin"
        );
    }
}

#[test]
fn a_public_base_url_with_a_path_prefix_is_refused() {
    let mut decoded = capabilities();
    decoded.public_base_url = "https://sync.example.invalid/base/".into();
    assert!(matches!(
        validate_capabilities(&endpoint(), &decoded).unwrap_err(),
        NativeProbeFailure::PublicBaseUrlMismatch { .. }
    ));
}

#[test]
fn endpoint_normalization_rejects_everything_that_would_repoint_a_request() {
    for raw in [
        // Userinfo would put a secret in the URL rather than a header.
        "https://user:pw@sync.example.invalid/",
        // A base path would silently prefix every control call.
        "https://sync.example.invalid/api/",
        "https://sync.example.invalid/?token=abc",
        "https://sync.example.invalid/#frag",
        // Cleartext needs a literal loopback address, not a DNS name.
        "http://sync.example.invalid/",
        "http://localhost:8790/",
        "http://localhost/",
        "http://127.0.0.1:0/",
        "ftp://sync.example.invalid/",
        "not a url",
    ] {
        assert!(
            NativeEndpoint::parse(raw).is_err(),
            "{raw} must be rejected as a native endpoint"
        );
    }
}

#[test]
fn endpoint_normalization_accepts_https_and_literal_loopback() {
    for raw in [
        "https://sync.example.invalid/",
        "https://sync.example.invalid:8443/",
        "http://127.0.0.1:8790/",
        "http://127.0.0.53:8790/",
        "http://[::1]:8790/",
    ] {
        assert!(
            NativeEndpoint::parse(raw).is_ok(),
            "{raw} must be accepted as a native endpoint"
        );
    }
}

#[test]
fn origins_separate_scheme_host_and_port() {
    assert_eq!(
        endpoint().origin(),
        (
            "https".to_string(),
            "sync.example.invalid".to_string(),
            None
        )
    );
    assert_eq!(
        NativeEndpoint::parse("https://sync.example.invalid:8443/")
            .unwrap()
            .origin(),
        (
            "https".to_string(),
            "sync.example.invalid".to_string(),
            Some(8443)
        )
    );
}

#[test]
fn api_keys_are_shape_checked_before_they_leave_the_process() {
    assert!(validate_api_key(&"a".repeat(32)).is_ok());
    assert!(validate_api_key(&"a".repeat(512)).is_ok());
    for key in [
        "",
        &"a".repeat(31),
        &"a".repeat(513),
        "has space",
        "has\nnewline",
    ] {
        assert!(
            matches!(
                validate_api_key(key),
                Err(NativeProbeFailure::ApiKey { .. })
            ),
            "{key:?} must be rejected"
        );
    }
}

#[test]
fn a_client_refuses_a_malformed_key_before_opening_a_connection() {
    let error = match NativeClient::new(&endpoint(), "too-short") {
        Ok(_) => panic!("a malformed key must not produce a client"),
        Err(error) => error,
    };
    assert!(matches!(error, SyncError::Unsupported(_)));
}

#[test]
fn a_conflict_keeps_the_detail_the_coordinator_classifies() {
    // The coordinator decides "reconcile and retry" from the rendered detail,
    // so the SDK's structured failure must still reach it in this exact shape.
    let conflict = map_sdk_error(sync_client::Error::Api {
        status: 409,
        failure: Box::new(sync_client::ServerFailure {
            code: "CONFLICT".into(),
            reason: "record changed".into(),
            record_name: None,
            zone: None,
            server_record: None,
        }),
    });
    assert!(matches!(conflict, SyncError::PreconditionFailed(_)));
    assert!(
        conflict
            .to_string()
            .contains("HTTP 409 CONFLICT: record changed"),
        "{conflict}"
    );
}

#[test]
fn the_sdk_error_model_maps_onto_the_shared_provider_errors() {
    assert!(matches!(
        map_sdk_error(sync_client::Error::Authentication("expired".into())),
        SyncError::Authentication
    ));
    assert!(matches!(
        map_sdk_error(sync_client::Error::NotFound("gone".into())),
        SyncError::NotFound(_)
    ));
    assert!(matches!(
        map_sdk_error(sync_client::Error::LeaseBusy),
        SyncError::RateLimited
    ));
    assert!(matches!(
        map_sdk_error(sync_client::Error::Protocol("bad".into())),
        SyncError::Protocol(_)
    ));
    assert!(matches!(
        map_sdk_error(sync_client::Error::Transport("offline".into())),
        SyncError::Offline(_)
    ));
    // A lease loss is not a protocol error: the cycle must end.
    assert!(matches!(
        map_sdk_error(sync_client::Error::LeaseLost),
        SyncError::Unsupported(_)
    ));
    assert!(map_sdk_error(sync_client::Error::ExpiredSyncToken)
        .to_string()
        .contains("EXPIRED_SYNC_TOKEN"));
}

#[test]
fn a_zone_is_validated_against_the_pinned_scope() {
    let pinned = validate_capabilities(&endpoint(), &capabilities()).unwrap();
    assert_eq!(
        validate_zone(&pinned, " PrimarySync ").unwrap(),
        NativeZone::new("PrimarySync")
    );
    assert_eq!(
        validate_zone(&pinned, "OtherSync").unwrap_err(),
        NativeProbeFailure::ZoneUnknown {
            zone: "OtherSync".into()
        }
    );
    assert!(validate_zone(&pinned, "  ").is_err());
}
