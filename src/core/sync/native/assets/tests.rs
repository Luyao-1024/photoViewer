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
    assert_eq!(AssetRole::Original.sdk(), sync_client::AssetRole::Original);
    assert_eq!(
        AssetRole::from_sdk(sync_client::AssetRole::Thumbnail),
        AssetRole::Thumbnail
    );
}

#[test]
fn a_reservation_declares_its_digest_as_lowercase_hex() {
    // The service validates the reservation digest against `[0-9a-f]{64}`.
    // Sending the Base64 a descriptor returns would be refused as a malformed
    // digest — and would look like a corrupt upload rather than a wrong
    // encoding.
    let sdk = sdk_digest(&digest(2048)).unwrap();
    assert_eq!(sdk.size(), 2048);
    assert_eq!(sdk.hex(), digest(2048).hex);
    assert_eq!(sdk.base64(), digest(2048).base64);

    let uppercase = ContentDigest {
        hex: digest(2048).hex.to_uppercase(),
        ..digest(2048)
    };
    assert!(sdk_digest(&uppercase).is_err());
    let base64 = ContentDigest {
        hex: digest(2048).base64,
        ..digest(2048)
    };
    assert!(sdk_digest(&base64).is_err());
}

#[test]
fn a_granted_reservation_keeps_its_session_and_asset() {
    let mapped = native_reservation(sync_client::AssetReservation {
        role: sync_client::AssetRole::Original,
        record_name: "master-1".into(),
        field_name: "resOriginalRes".into(),
        session_id: "aaaa".into(),
        state: sync_client::SessionState::Reserved,
        url: Some("https://s.invalid/v1/uploads/aaaa/content?expires=1&token=t".into()),
        asset: None,
    });
    assert_eq!(mapped.role, AssetRole::Original);
    assert_eq!(mapped.session_id, "aaaa");
    assert_eq!(mapped.state, SessionState::Reserved);
    assert!(mapped.asset.is_none());
    // The capability URL is carried through for the transfer, not persisted.
    assert!(mapped.url.contains("token=t"));
    assert_eq!(
        sdk_reservation(&mapped).state,
        sync_client::SessionState::Reserved
    );
}

#[test]
fn a_published_reservation_carries_the_only_trustworthy_asset_id() {
    let mapped = native_reservation(sync_client::AssetReservation {
        role: sync_client::AssetRole::Thumbnail,
        record_name: "master-1".into(),
        field_name: "resJPEGThumbRes".into(),
        session_id: "bbbb".into(),
        state: sync_client::SessionState::Published,
        url: None,
        asset: Some(sync_client::AssetDescriptor {
            asset_id: "bbbb".into(),
            size: 71,
            file_checksum: "N4kXJS+f44FaFFqy99mQC2jGiVb2+sBo/cnZh7fnpBk=".into(),
            checksum_algorithm: "SHA256".into(),
            mime_type: "image/png".into(),
            width: Some(2),
            height: Some(2),
            download_url: None,
            receipt: None,
        }),
    });
    assert_eq!(mapped.state, SessionState::Published);
    let asset = mapped.asset.unwrap();
    assert_eq!(asset.asset_id, "bbbb");
    assert_eq!(asset.size, 71);
    assert_eq!(asset.mime_type, "image/png");
    assert_eq!(mapped.url, "");
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
    let checksum = transfer_error(sync_client::Error::ChecksumMismatch {
        expected: "aa".into(),
        actual: "bb".into(),
    });
    assert!(matches!(checksum, TransferError::ChecksumMismatch(_)));
    // The bytes are gone either way, so a mismatch is not retried in place.
    assert!(checksum.to_string().contains("checksum"));

    assert!(matches!(
        transfer_error(sync_client::Error::SizeMismatch {
            expected: 71,
            actual: 70,
        }),
        TransferError::SizeMismatch(_)
    ));
    for (code, reason) in [
        ("CHECKSUM_MISMATCH", "digest differs"),
        ("UPLOAD_SIZE_MISMATCH", "short"),
        ("INVALID_THUMBNAIL", "too wide"),
        ("UPLOAD_TIMEOUT", "idle"),
    ] {
        let mapped = transfer_error(sync_client::Error::UploadRejected {
            code: code.into(),
            reason: reason.into(),
        });
        if code == "CHECKSUM_MISMATCH" {
            assert!(matches!(mapped, TransferError::ChecksumMismatch(_)));
        } else if code == "UPLOAD_SIZE_MISMATCH" {
            assert!(matches!(mapped, TransferError::SizeMismatch(_)));
        } else if code == "INVALID_THUMBNAIL" {
            assert!(matches!(mapped, TransferError::InvalidThumbnail(_)));
        } else {
            assert!(matches!(mapped, TransferError::SessionFailed(_)));
        }
    }
    assert!(matches!(
        transfer_error(sync_client::Error::UploadRejected {
            code: "UPLOAD_LEASE_LOST".into(),
            reason: "gone".into(),
        }),
        TransferError::LeaseLost
    ));
    assert!(matches!(
        transfer_error(sync_client::Error::LeaseLost),
        TransferError::LeaseLost
    ));
    assert!(matches!(
        transfer_error(sync_client::Error::UploadFailed("session gone".into())),
        TransferError::SessionFailed(_)
    ));
    // An unrecognised code is not guessed at.
    assert!(matches!(
        transfer_error(sync_client::Error::UploadRejected {
            code: "INTERNAL_ERROR".into(),
            reason: "boom".into(),
        }),
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
