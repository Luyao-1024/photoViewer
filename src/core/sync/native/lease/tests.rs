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
fn a_renewal_response_is_owned_by_the_sdk_test_suite() {
    // The SDK parses renewals too: the local client only records the returned
    // TTL against the instant the request left.
}

#[test]
fn a_proof_crosses_to_the_sdk_without_reformatting_the_fence() {
    let proof = grant(90, 30).proof();
    let sdk = proof.sdk();
    assert_eq!(sdk.lease_id, "11111111-2222-4333-8444-555555555555");
    assert_eq!(sdk.lease_token, "sealed-token");
    assert_eq!(sdk.fencing_token, "7");
}

#[test]
fn phases_round_trip_through_the_sdk() {
    for phase in UploadPhase::ORDER {
        assert_eq!(UploadPhase::from_sdk(phase.sdk()), phase);
        assert_eq!(phase.sdk().as_str(), phase.as_str());
    }
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
