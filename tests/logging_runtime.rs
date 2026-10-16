//! Functional process logging and persisted controls; no GTK/UX interaction.
use photo_viewer::{
    config,
    core::{diagnostics, prefs},
};
#[test]
fn persisted_levels_quota_and_log_cleanup_apply_to_the_running_writer() {
    let _sandbox = photo_viewer::testing::Sandbox::install();
    let guard = diagnostics::init().unwrap();
    diagnostics::set_log_level("info").unwrap();
    tracing::info!(target:"logging_functional", "before-filter-change");
    diagnostics::flush_logs().unwrap();
    let path = config::cache_dir().join("logs/app.log");
    assert!(std::fs::read_to_string(&path)
        .unwrap()
        .contains("before-filter-change"));
    diagnostics::set_log_level("error").unwrap();
    tracing::info!(target:"logging_functional", "suppressed-info");
    tracing::error!(target:"logging_functional", "retained-error");
    diagnostics::flush_logs().unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(!text.contains("suppressed-info"));
    assert!(text.contains("retained-error"));
    assert_eq!(prefs::log_level(), "error");
    diagnostics::set_log_level("debug").unwrap();
    tracing::debug!(target:"logging_functional", "retained-debug");
    diagnostics::flush_logs().unwrap();
    assert!(std::fs::read_to_string(&path)
        .unwrap()
        .contains("retained-debug"));
    diagnostics::set_log_limit_mb(900).unwrap();
    assert_eq!(prefs::log_limit_mb(), 500);
    diagnostics::set_log_limit_mb(1).unwrap();
    assert_eq!(prefs::log_limit_mb(), 1);
    assert!(diagnostics::log_size() <= 1_000_000);
    diagnostics::clear_logs().unwrap();
    tracing::info!(target:"logging_functional", "after-clear");
    diagnostics::flush_logs().unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("after-clear"));
    assert!(!text.contains("before-filter-change"));
    let resolved = std::fs::canonicalize(&path).unwrap();
    assert!(resolved
        .file_name()
        .unwrap()
        .to_string_lossy()
        .ends_with(".log"));
    assert!(resolved
        .parent()
        .unwrap()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .starts_with("20"));
    assert!(diagnostics::log_size() <= 1_000_000);
    drop(guard);
}
