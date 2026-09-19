use super::model::{
    Baseline, ConflictKind, EntrySnapshot, Observation, PlanAction, RevisionStrength, SyncDirection,
};

pub fn plan(entry: &EntrySnapshot) -> PlanAction {
    if matches!(entry.local, Observation::Unknown) || matches!(entry.remote, Observation::Unknown) {
        return PlanAction::WaitForCompleteObservation;
    }

    match &entry.baseline {
        None => plan_without_baseline(entry),
        Some(baseline) => plan_with_baseline(entry, baseline),
    }
}

fn plan_without_baseline(entry: &EntrySnapshot) -> PlanAction {
    match (&entry.local, &entry.remote) {
        (Observation::Absent, Observation::Absent) => PlanAction::Noop,
        (Observation::Present { .. }, Observation::Absent) => match entry.direction {
            SyncDirection::Bidirectional | SyncDirection::UploadOnly => PlanAction::UploadNew,
            SyncDirection::DownloadOnly => PlanAction::Noop,
        },
        (Observation::Absent, Observation::Present { .. }) => match entry.direction {
            SyncDirection::Bidirectional | SyncDirection::DownloadOnly => PlanAction::DownloadNew,
            SyncDirection::UploadOnly => PlanAction::Noop,
        },
        (Observation::Present { .. }, Observation::Present { .. }) => {
            if observations_have_equal_fingerprints(&entry.local, &entry.remote) {
                PlanAction::Noop
            } else if entry.local.fingerprint().is_some() && entry.remote.fingerprint().is_some() {
                PlanAction::Conflict(ConflictKind::InitialContentMismatch)
            } else {
                PlanAction::VerifyContent
            }
        }
        (Observation::Unknown, _) | (_, Observation::Unknown) => {
            PlanAction::WaitForCompleteObservation
        }
    }
}

fn plan_with_baseline(entry: &EntrySnapshot, baseline: &Baseline) -> PlanAction {
    let local_changed = side_changed(&entry.local, baseline, true);
    let remote_changed = side_changed(&entry.remote, baseline, false);

    match (&entry.local, &entry.remote) {
        (Observation::Absent, Observation::Absent) => PlanAction::Noop,
        (Observation::Absent, Observation::Present { .. }) => {
            if !entry.propagate_deletes {
                return PlanAction::Conflict(ConflictKind::MissingWithDeletesDisabled);
            }
            if remote_changed {
                return PlanAction::Conflict(ConflictKind::DeleteVsModify);
            }
            match entry.direction {
                SyncDirection::Bidirectional | SyncDirection::UploadOnly => {
                    let Some(revision) = entry.remote.revision().cloned() else {
                        return PlanAction::Conflict(ConflictKind::InsufficientEvidence);
                    };
                    if revision.strength != RevisionStrength::Strong {
                        return PlanAction::Conflict(ConflictKind::InsufficientEvidence);
                    }
                    PlanAction::DeleteRemote { expected: revision }
                }
                SyncDirection::DownloadOnly => PlanAction::DownloadNew,
            }
        }
        (Observation::Present { .. }, Observation::Absent) => {
            if !entry.propagate_deletes {
                return PlanAction::Conflict(ConflictKind::MissingWithDeletesDisabled);
            }
            if local_changed {
                return PlanAction::Conflict(ConflictKind::DeleteVsModify);
            }
            match entry.direction {
                SyncDirection::Bidirectional | SyncDirection::DownloadOnly => {
                    PlanAction::DeleteLocal
                }
                SyncDirection::UploadOnly => PlanAction::UploadNew,
            }
        }
        (Observation::Present { .. }, Observation::Present { .. }) => {
            if observations_have_equal_fingerprints(&entry.local, &entry.remote) {
                return PlanAction::Noop;
            }
            match (local_changed, remote_changed) {
                (false, false) => PlanAction::VerifyContent,
                (true, false) => match entry.direction {
                    SyncDirection::Bidirectional | SyncDirection::UploadOnly => {
                        let Some(expected) = entry.remote.revision().cloned() else {
                            return PlanAction::Conflict(ConflictKind::InsufficientEvidence);
                        };
                        if expected.strength != RevisionStrength::Strong {
                            return PlanAction::Conflict(ConflictKind::InsufficientEvidence);
                        }
                        PlanAction::UploadReplace { expected }
                    }
                    SyncDirection::DownloadOnly => PlanAction::Conflict(ConflictKind::BothModified),
                },
                (false, true) => match entry.direction {
                    SyncDirection::Bidirectional | SyncDirection::DownloadOnly => {
                        PlanAction::DownloadReplace
                    }
                    SyncDirection::UploadOnly => PlanAction::Conflict(ConflictKind::BothModified),
                },
                (true, true) => PlanAction::Conflict(ConflictKind::BothModified),
            }
        }
        (Observation::Unknown, _) | (_, Observation::Unknown) => {
            PlanAction::WaitForCompleteObservation
        }
    }
}

fn observations_have_equal_fingerprints(left: &Observation, right: &Observation) -> bool {
    matches!((left.fingerprint(), right.fingerprint()), (Some(a), Some(b)) if a == b)
}

fn side_changed(observation: &Observation, baseline: &Baseline, local: bool) -> bool {
    match observation {
        Observation::Unknown | Observation::Absent => true,
        Observation::Present {
            fingerprint,
            revision,
            ..
        } => {
            if let Some(fingerprint) = fingerprint {
                return fingerprint != &baseline.fingerprint;
            }
            let baseline_revision = if local {
                baseline.local_revision.as_ref()
            } else {
                baseline.remote_revision.as_ref()
            };
            match (revision, baseline_revision) {
                (Some(current), Some(previous)) => current != previous,
                _ => true,
            }
        }
    }
}

#[cfg(test)]
mod tests;
