use super::*;
use crate::core::sync::model::{Fingerprint, Revision};

fn present(hash: &str, revision: &str) -> Observation {
    Observation::Present {
        fingerprint: Some(Fingerprint {
            size: 10,
            blake3: hash.into(),
        }),
        revision: Some(Revision::etag(revision)),
        size: 10,
        modified_unix: None,
    }
}

fn snapshot(local: Observation, remote: Observation, baseline: Option<Baseline>) -> EntrySnapshot {
    EntrySnapshot {
        relative_path: "photo.jpg".into(),
        local,
        remote,
        baseline,
        direction: SyncDirection::Bidirectional,
        propagate_deletes: false,
    }
}

fn baseline(hash: &str) -> Baseline {
    Baseline {
        fingerprint: Fingerprint {
            size: 10,
            blake3: hash.into(),
        },
        local_revision: None,
        remote_revision: Some(Revision::etag("\"old\"")),
    }
}

#[test]
fn unknown_never_becomes_a_destructive_action() {
    let action = plan(&snapshot(
        Observation::Unknown,
        present("remote", "\"v1\""),
        Some(baseline("old")),
    ));
    assert_eq!(action, PlanAction::WaitForCompleteObservation);
}

#[test]
fn first_local_file_is_uploaded_without_overwrite() {
    let action = plan(&snapshot(
        present("local", "local"),
        Observation::Absent,
        None,
    ));
    assert_eq!(action, PlanAction::UploadNew);
}

#[test]
fn both_changed_is_a_conflict() {
    let action = plan(&snapshot(
        present("local-v2", "local-v2"),
        present("remote-v2", "\"remote-v2\""),
        Some(baseline("v1")),
    ));
    assert_eq!(action, PlanAction::Conflict(ConflictKind::BothModified));
}

#[test]
fn deletion_is_not_reversed_when_propagation_is_disabled() {
    let action = plan(&snapshot(
        Observation::Absent,
        present("v1", "\"v1\""),
        Some(baseline("v1")),
    ));
    assert_eq!(
        action,
        PlanAction::Conflict(ConflictKind::MissingWithDeletesDisabled)
    );
}

#[test]
fn weak_etag_cannot_authorize_replacement() {
    let action = plan(&snapshot(
        present("v2", "local-v2"),
        present("v1", "W/\"v1\""),
        Some(baseline("v1")),
    ));
    assert_eq!(
        action,
        PlanAction::Conflict(ConflictKind::InsufficientEvidence)
    );
}
