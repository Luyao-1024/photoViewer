use super::*;
use crate::core::sync::content::ContentDigest;

fn digest(size: u64) -> ContentDigest {
    ContentDigest {
        size,
        hex: "378917252f9fe3815a145ab2f7d9900b68c68956f6fac068fdc9d987b7e7a419".into(),
        base64: "N4kXJS+f44FaFFqy99mQC2jGiVb2+sBo/cnZh7fnpBk=".into(),
    }
}

#[test]
fn a_role_binds_to_exactly_one_master_field() {
    assert_eq!(AssetRole::Original.field_name(), "resOriginalRes");
    assert_eq!(AssetRole::Thumbnail.field_name(), "resJPEGThumbRes");
    assert_eq!(AssetRole::Original.as_str(), "original");
    assert_eq!(AssetRole::Thumbnail.as_str(), "thumbnail");
}

#[test]
fn a_reservation_declares_its_digest_as_lowercase_hex() {
    // The service validates this against `[0-9a-f]{64}`. Sending the Base64 a
    // descriptor returns would be refused as a malformed digest — and would
    // look like a corrupt upload rather than a wrong encoding.
    let request = ReservationRequest {
        role: AssetRole::Original,
        intent: UploadIntent::Create,
        digest: digest(2048),
    };
    let wire = ReserveToken {
        record_name: "master-1",
        record_type: super::super::receive::MASTER_RECORD_TYPE,
        field_name: request.role.field_name(),
        options: ReserveOptions {
            role: request.role,
            intent: request.intent,
            expected_size: request.digest.size,
            expected_sha256: &request.digest.hex,
        },
    };
    let encoded = serde_json::to_string(&wire).unwrap();
    assert!(
        encoded.contains("\"recordType\":\"CPLMaster\""),
        "{encoded}"
    );
    assert!(
        encoded.contains("\"fieldName\":\"resOriginalRes\""),
        "{encoded}"
    );
    assert!(encoded.contains("\"role\":\"original\""), "{encoded}");
    assert!(encoded.contains("\"intent\":\"create\""), "{encoded}");
    assert!(encoded.contains("\"expectedSize\":2048"), "{encoded}");
    assert!(
        encoded.contains("\"expectedSha256\":\"378917252f9fe3815a145ab2f7d9900b68c68956f6fac068fdc9d987b7e7a419\""),
        "{encoded}"
    );
    assert!(
        !encoded.contains("N4kXJS"),
        "base64 must not reach a reservation"
    );
}

#[test]
fn a_granted_reservation_is_read_from_the_tokens_own_extension() {
    let response: ReserveResponse = serde_json::from_str(
        r#"{"tokens":[
            {"recordName":"master-1","fieldName":"resOriginalRes",
             "url":"https://s.invalid/v1/uploads/aaaa/content?expires=1&token=t",
             "_syncserver":{"sessionId":"aaaa","state":"Reserved"}},
            {"recordName":"master-1","fieldName":"resJPEGThumbRes",
             "url":"https://s.invalid/v1/uploads/bbbb/content?expires=1&token=t",
             "_syncserver":{"sessionId":"bbbb","state":"Reserved"}}
        ]}"#,
    )
    .unwrap();
    assert_eq!(response.tokens.len(), 2);
    assert_eq!(
        response.tokens[0].options.as_ref().unwrap().session_id,
        "aaaa"
    );
    assert_eq!(
        response.tokens[0].options.as_ref().unwrap().state,
        SessionState::Reserved
    );
    // The role is recovered from the field, not from ordering.
    assert_eq!(
        role_from_field(&response.tokens[0].field_name),
        AssetRole::Original
    );
    assert_eq!(
        role_from_field(&response.tokens[1].field_name),
        AssetRole::Thumbnail
    );
}

#[test]
fn session_states_distinguish_success_from_a_terminal_failure() {
    // Only Published carries an asset id; every other state is "still nothing".
    assert!(SessionState::Published.is_published());
    for state in [
        SessionState::Reserved,
        SessionState::Receiving,
        SessionState::PublishPrepared,
        SessionState::Failed,
        SessionState::Expired,
    ] {
        assert!(!state.is_published(), "{state:?}");
    }
    // Failed and Expired both mean the bytes are gone: there is no resume, so
    // the recovery is a new reservation rather than another attempt.
    assert!(SessionState::Failed.is_terminal_failure());
    assert!(SessionState::Expired.is_terminal_failure());
    assert!(!SessionState::Reserved.is_terminal_failure());
}

#[test]
fn an_upload_answer_names_its_result_under_single_file() {
    // There is no top-level assetId: a client reading one would see nothing.
    let response: UploadResponse = serde_json::from_str(
        r#"{"singleFile":{"assetId":"bbbb","size":71,
             "fileChecksum":"N4kXJS+f44FaFFqy99mQC2jGiVb2+sBo/cnZh7fnpBk=",
             "checksumAlgorithm":"SHA256","mimeType":"image/png","width":2,"height":2}}"#,
    )
    .unwrap();
    assert_eq!(response.single_file.asset_id, "bbbb");
    assert_eq!(response.single_file.size, 71);
    assert_eq!(
        response.single_file.file_checksum,
        "N4kXJS+f44FaFFqy99mQC2jGiVb2+sBo/cnZh7fnpBk="
    );
    assert_eq!(response.single_file.mime_type, "image/png");
}

#[test]
fn a_described_asset_carries_a_fresh_capability() {
    let described: DownloadDescriptor = serde_json::from_str(
        r#"{"assetId":"bbbb","size":71,
             "fileChecksum":"N4kXJS+f44FaFFqy99mQC2jGiVb2+sBo/cnZh7fnpBk=",
             "checksumAlgorithm":"SHA256","mimeType":"image/png",
             "downloadURL":"https://s.invalid/v1/assets/bbbb/content?role=original&expires=9&token=t",
             "receipt":"sealed"}"#,
    )
    .unwrap();
    assert_eq!(described.asset_id(), "bbbb");
    assert!(described.download_url.contains("expires="));
    assert_eq!(described.receipt, "sealed");
}

#[test]
fn transfer_failures_map_onto_their_recoveries() {
    let checksum = transfer_error(
        400,
        br#"{"serverErrorCode":"CHECKSUM_MISMATCH","reason":"digest differs"}"#,
    );
    assert!(matches!(checksum, TransferError::ChecksumMismatch(_)));
    // The bytes are gone either way, so a mismatch is not retried in place.
    assert!(checksum.to_string().contains("CHECKSUM") || checksum.to_string().contains("digest"));

    assert!(matches!(
        transfer_error(
            400,
            br#"{"serverErrorCode":"UPLOAD_SIZE_MISMATCH","reason":"short"}"#
        ),
        TransferError::SizeMismatch(_)
    ));
    assert!(matches!(
        transfer_error(
            400,
            br#"{"serverErrorCode":"INVALID_THUMBNAIL","reason":"too wide"}"#
        ),
        TransferError::InvalidThumbnail(_)
    ));
    assert!(matches!(
        transfer_error(
            409,
            br#"{"serverErrorCode":"UPLOAD_LEASE_LOST","reason":"gone"}"#
        ),
        TransferError::LeaseLost
    ));
    assert!(matches!(
        transfer_error(
            408,
            br#"{"serverErrorCode":"UPLOAD_TIMEOUT","reason":"idle"}"#
        ),
        TransferError::SessionFailed(_)
    ));
    // An unrecognised code is not guessed at.
    assert!(matches!(
        transfer_error(
            500,
            br#"{"serverErrorCode":"INTERNAL_ERROR","reason":"boom"}"#
        ),
        TransferError::Transport(_)
    ));
}

#[test]
fn a_lease_loss_is_reported_as_unsupported_so_the_cycle_ends() {
    // The coordinator decides "stop the cycle" from this variant, so the mapping
    // has to survive the trip through the shared error type.
    let error: crate::core::sync::SyncError = TransferError::LeaseLost.into();
    assert!(
        matches!(error, crate::core::sync::SyncError::Unsupported(message)
        if message.contains(super::super::lease::UPLOAD_LEASE_LOST))
    );
}

#[test]
fn only_a_partial_answer_continues_a_resume() {
    // A 200 in answer to a ranged request means the asset changed; appending
    // would splice two versions together.
    let full = AssetBytes {
        bytes: vec![1, 2, 3],
        etag: Some("\"sha256:aa\"".into()),
        partial: false,
        content_range: None,
    };
    assert!(!full.continues());
    let partial = AssetBytes {
        partial: true,
        content_range: Some("bytes 3-5/6".into()),
        ..full.clone()
    };
    assert!(partial.continues());
}
