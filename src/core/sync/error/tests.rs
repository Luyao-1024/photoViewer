use super::*;

/// The detail a real quota rejection produced, verbatim. This is the whole
/// reason `classify_detail` exists: the cause travelled all the way to the
/// overview row inside this string, and the row used to call it a version
/// conflict.
const QUOTA_DETAIL: &str = "截图/2026-10-16.png: transport failed: remote version changed: \
     HTTP 409 STORAGE_QUOTA_EXCEEDED: Account storage quota exceeded";

#[test]
fn quota_rejection_is_named_rather_than_called_a_version_change() {
    assert_eq!(classify_detail(QUOTA_DETAIL), SyncCause::Quota);
}

#[test]
fn quota_is_something_only_the_user_can_unblock() {
    assert_eq!(SyncCause::Quota.class(), SyncClass::NeedsUser);
    assert_eq!(SyncCause::Credential.class(), SyncClass::NeedsUser);
    assert_eq!(SyncCause::Incompatible.class(), SyncClass::Unsupported);
}

#[test]
fn every_service_code_lands_in_some_bucket() {
    for (code, _) in SERVICE_CODES {
        let detail = format!("HTTP 409 {code}: whatever the service said");
        assert_ne!(
            classify_detail(&detail),
            SyncCause::Unknown,
            "{code} classified as unknown instead of its own bucket"
        );
    }
}

#[test]
fn codes_that_used_to_share_one_message_are_told_apart() {
    // These all arrived as "remote version changed" and all mean something
    // different to the reader.
    let cases = [
        (
            "HTTP 409 PATH_CONFLICT: same name, different photo",
            SyncCause::Conflict,
        ),
        (
            "HTTP 409 IDEMPOTENCY_CONFLICT: entry was reused",
            SyncCause::Conflict,
        ),
        (
            "HTTP 409 UPLOAD_STATE_CONFLICT: phase moved",
            SyncCause::Conflict,
        ),
        (
            "HTTP 409 UNSUPPORTED_CAPABILITY: no such field",
            SyncCause::Incompatible,
        ),
        (
            "HTTP 401 AUTHENTICATION_REQUIRED: key revoked",
            SyncCause::Credential,
        ),
        ("HTTP 503 SERVER_BUSY: try later", SyncCause::ServerBusy),
        (
            "HTTP 400 CHECKSUM_MISMATCH: bytes differ",
            SyncCause::Checksum,
        ),
    ];
    for (detail, expected) in cases {
        assert_eq!(classify_detail(detail), expected, "{detail}");
    }
}

#[test]
fn transport_failures_are_recognised_without_a_service_code() {
    assert_eq!(
        classify_detail("network is unavailable: connection refused"),
        SyncCause::Network
    );
    assert_eq!(
        classify_detail("authentication failed"),
        SyncCause::Credential
    );
    assert_eq!(
        classify_detail("remote service is rate limiting requests"),
        SyncCause::ServerBusy
    );
    assert_eq!(
        classify_detail("provider capability is unsupported: HTTP 413"),
        SyncCause::Incompatible
    );
}

#[test]
fn a_5xx_detail_is_retryable_rather_than_a_protocol_fault() {
    // `Protocol` reads as permanent — an unparsable body or a version we do
    // not speak — so a busy server must not be filed under it.
    let busy = SyncError::Protocol("HTTP 503 SERVER_BUSY: try later".into());
    assert_eq!(busy.class(), SyncClass::Retryable);
    assert_eq!(busy.cause(), SyncCause::ServerBusy);
}

#[test]
fn text_no_bucket_claims_stays_unknown() {
    assert_eq!(
        classify_detail("something nobody anticipated"),
        SyncCause::Unknown
    );
    assert_eq!(SyncCause::Unknown.class(), SyncClass::Retryable);
}

#[test]
fn headline_and_detail_point_at_real_keys() {
    // Every cause must resolve to a key that exists in every locale; the
    // i18n completeness test only checks keys it can see.
    for cause in [
        SyncCause::Quota,
        SyncCause::Network,
        SyncCause::Credential,
        SyncCause::ServerBusy,
        SyncCause::Conflict,
        SyncCause::Incompatible,
        SyncCause::Checksum,
        SyncCause::Unknown,
    ] {
        assert!(
            crate::core::i18n::tr(cause.headline_key()) != cause.headline_key(),
            "{} has no headline translation",
            cause.headline_key()
        );
        assert!(
            crate::core::i18n::tr(cause.detail_key()) != cause.detail_key(),
            "{} has no detail translation",
            cause.detail_key()
        );
    }
}
