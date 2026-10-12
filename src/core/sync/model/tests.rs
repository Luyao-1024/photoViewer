use super::*;

#[test]
fn interrupted_run_reason_round_trips_without_localized_copy() {
    let reason = SyncFailureReason::InterruptedRun;
    assert_eq!(reason.as_str(), "sync.interrupted_run");
    assert!(reason.as_str().is_ascii());
    assert_eq!(SyncFailureReason::parse(reason.as_str()), Some(reason));
}

#[test]
fn interrupted_run_reason_recognizes_legacy_persisted_copy() {
    assert_eq!(
        SyncFailureReason::parse("上次同步被中断（应用在同步完成前退出），已标记为失败，可重试。"),
        Some(SyncFailureReason::InterruptedRun)
    );
}

#[test]
fn interrupted_run_reason_does_not_classify_other_diagnostics() {
    for error in [
        "",
        "sync.unknown_reason",
        "HTTP 503 Service Unavailable",
        "server returned sync.interrupted_run in its response body",
    ] {
        assert_eq!(SyncFailureReason::parse(error), None, "{error:?}");
    }
}
