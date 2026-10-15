use super::*;
use std::time::{Duration, Instant};

fn grant(ttl: u64, heartbeat: u64) -> LeaseGrant {
    LeaseGrant {
        lease_id: "11111111-2222-4333-8444-555555555555".into(),
        lease_token: "sealed-token".into(),
        fencing_token: "7".into(),
        ttl_seconds: ttl,
        heartbeat_seconds: heartbeat,
        phase: "checking".into(),
    }
}

#[test]
fn a_grant_yields_the_proof_the_service_issued() {
    let grant = grant(90, 30);
    let proof = grant.proof();
    assert_eq!(proof.lease_id, grant.lease_id);
    assert_eq!(proof.lease_token, grant.lease_token);
    // The fence stays text: the service compares it as a canonical decimal, so
    // reformatting it would change which grant it names.
    assert_eq!(proof.fencing_token, "7");

    let headers = lease_headers(&proof);
    assert_eq!(headers.len(), 3, "exactly one header of each kind");
    assert_eq!(headers[0].0, "X-Sync-Lease-ID");
    assert_eq!(headers[1].0, "X-Sync-Lease-Token");
    assert_eq!(headers[2].0, "X-Sync-Lease-Fence");
}

#[tokio::test]
async fn the_deadline_holds_back_a_margin_of_the_granted_ttl() {
    let started = Instant::now();
    let lease = UploadLease::from_grant(&grant(90, 30), started);
    assert_eq!(lease.granted_ttl(), Duration::from_secs(90));
    assert_eq!(lease.heartbeat_interval(), Duration::from_secs(30));

    let held = lease.deadline().await.expect("held") - started;
    // Never the full TTL: a request issued just before expiry must not still be
    // assumed live after it.
    assert!(held < Duration::from_secs(90), "{held:?}");
    assert!(held > Duration::from_secs(80), "{held:?}");
}

#[tokio::test]
async fn a_renewal_extends_from_when_the_request_started() {
    let started = Instant::now();
    let lease = UploadLease::from_grant(&grant(90, 30), started);
    let original = lease.deadline().await.unwrap();

    // A renewal that left at T and granted 90s extends the window from T, not
    // from the moment the answer came back.
    let sent_at = started + Duration::from_secs(30);
    lease.renew(90, sent_at).await;
    let after = lease.deadline().await.unwrap();
    assert!(after > original, "{after:?} should exceed {original:?}");
    assert_eq!(after, sent_at + Duration::from_secs(81));
}

#[test]
fn a_renewal_that_returns_after_the_deadline_cannot_revive_the_lease() {
    let started = Instant::now();
    let current = started + Duration::from_secs(81);
    let held = LeaseState::Held { deadline: current };

    // The answer arrives well past the deadline, however generous its grant: a
    // slow response cannot hand back time the server has already given away.
    let late = renewed(
        held,
        started + Duration::from_secs(600),
        started + Duration::from_secs(60),
        90,
    );
    assert_eq!(late, LeaseState::Lost);

    // The same grant answered in time is kept.
    let on_time = renewed(
        held,
        started + Duration::from_secs(61),
        started + Duration::from_secs(60),
        90,
    );
    assert_eq!(
        on_time,
        LeaseState::Held {
            deadline: started + Duration::from_secs(141)
        }
    );

    // And nothing revives a lease already known to be lost.
    assert_eq!(
        renewed(LeaseState::Lost, started, started, 90),
        LeaseState::Lost
    );
}

#[tokio::test]
async fn a_renewal_never_shortens_an_existing_window() {
    let started = Instant::now();
    let lease = UploadLease::from_grant(&grant(90, 30), started);
    let long = lease.deadline().await.unwrap();
    lease.renew(10, started + Duration::from_secs(5)).await;
    let after = lease.deadline().await.unwrap();
    assert!(
        after >= long,
        "a shorter grant must not pull the deadline in"
    );
}

#[tokio::test]
async fn a_lost_lease_reports_no_deadline_at_all() {
    let lease = UploadLease::from_grant(&grant(90, 30), Instant::now());
    lease.mark_lost().await;
    assert!(!lease.is_held().await);
    assert!(lease.deadline().await.is_none());
    // A renewal after a loss is ignored: the cycle cannot recover authority it
    // has already been told it does not have.
    lease.renew(90, Instant::now()).await;
    assert!(lease.deadline().await.is_none());
}

#[test]
fn the_phase_ladder_walks_forward_and_stops() {
    assert_eq!(UploadPhase::ORDER[0], UploadPhase::Delete);
    assert_eq!(UploadPhase::Delete.next(), Some(UploadPhase::Modify));
    assert_eq!(UploadPhase::Modify.next(), Some(UploadPhase::Create));
    // There is no phase after create; advancing past it would be a client bug
    // the service rejects as a mismatch.
    assert_eq!(UploadPhase::Create.next(), None);
    assert_eq!(UploadPhase::Delete.as_str(), "delete");
    assert_eq!(UploadPhase::Modify.as_str(), "modify");
    assert_eq!(UploadPhase::Create.as_str(), "create");
}

#[test]
fn a_lease_check_decodes_the_keys_the_service_returns() {
    let check: LeaseCheck = serde_json::from_str(
        r#"{"isCurrent":true,"mayUpload":true,"zoneTokens":[{"zoneID":{"zoneName":"PrimarySync","ownerRecordName":"_defaultOwner"},"syncToken":"zone.abc"}],"phase":"delete"}"#,
    )
    .unwrap();
    assert!(check.is_current);
    assert!(check.may_upload);
    assert_eq!(check.phase, "delete");
    assert_eq!(check.zone_tokens[0].zone.zone_name, "PrimarySync");
    assert_eq!(check.zone_tokens[0].sync_token, "zone.abc");
}

#[test]
fn a_gate_that_has_not_passed_reports_no_permission_to_upload() {
    let check: LeaseCheck = serde_json::from_str(
        r#"{"isCurrent":false,"mayUpload":false,"zoneTokens":[],"phase":"checking"}"#,
    )
    .unwrap();
    assert!(!check.is_current);
    assert!(!check.may_upload);
    // The phase is still `checking`: a successful check is the only exit from
    // it, so a client that skipped the gate cannot slip into a phase.
    assert_eq!(check.phase, "checking");
}

#[test]
fn a_renewal_response_carries_no_heartbeat_and_the_client_does_not_invent_one() {
    // The heartbeat interval comes from the grant. Reading it from a renewal
    // that omits it would silently reset the client's schedule to a default.
    let response: RenewResponse =
        serde_json::from_str(r#"{"leaseId":"x","expiresAtMs":1,"ttlSeconds":90}"#).unwrap();
    assert_eq!(response.ttl_seconds, 90);
}

#[test]
fn lease_failure_codes_map_to_the_decision_they_imply() {
    // Acquisition contention and loss arrive as distinct codes and must not
    // collapse: one skips the cycle's uplink, the other ends it.
    assert!(matches!(
        lease_error(
            409,
            br#"{"serverErrorCode":"UPLOAD_LOCK_BUSY","reason":"held"}"#
        ),
        SyncError::RateLimited
    ));
    assert!(matches!(
        lease_error(409, br#"{"serverErrorCode":"UPLOAD_LEASE_LOST","reason":"gone"}"#),
        SyncError::Unsupported(message) if message.contains(UPLOAD_LEASE_LOST)
    ));
    assert!(matches!(
        lease_error(
            409,
            br#"{"serverErrorCode":"SYNC_NOT_CURRENT","reason":"behind"}"#
        ),
        SyncError::PreconditionFailed(_)
    ));
    // Anything unrecognised falls back to the shared status mapping rather than
    // being guessed at.
    assert!(matches!(
        lease_error(
            500,
            br#"{"serverErrorCode":"INTERNAL_ERROR","reason":"boom"}"#
        ),
        SyncError::Protocol(_)
    ));
}

#[test]
fn a_missing_lease_header_is_a_loss_not_a_bad_request() {
    // The service reports a missing proof the same way it reports a lost one, so
    // the client must not treat it as a request bug and retry.
    assert!(matches!(
        lease_error(
            409,
            br#"{"serverErrorCode":"UPLOAD_LEASE_LOST","reason":"Upload lease is absent, expired or replaced"}"#
        ),
        SyncError::Unsupported(_)
    ));
}

#[tokio::test]
async fn a_short_grant_remains_bounded_by_its_actual_ttl() {
    let started = Instant::now();
    // Keep subsecond usable time without inventing authority past the TTL.
    let lease = UploadLease::from_grant(&grant(1, 1), started);
    let held = lease.deadline().await.unwrap() - started;
    assert!(
        held > Duration::ZERO && held < Duration::from_secs(1),
        "{held:?}"
    );
}
