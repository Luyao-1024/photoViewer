use super::*;
use tracing_subscriber::{prelude::*, EnvFilter};
fn emit() {
    tracing::info!(target: "logging_test", "info event");
    tracing::debug!(target: "logging_test", password="private-value", "debug event");
}
#[test]
fn levels_reload_without_restart_and_clearing_keeps_logging_alive() {
    let dir = tempfile::tempdir().unwrap();
    let (logger, worker) = Logger::start(dir.path(), 1_000_000).unwrap().unwrap();
    let (filter, reload) = tracing_subscriber::reload::Layer::new(EnvFilter::new("info"));
    let subscriber = tracing_subscriber::registry()
        .with(filter)
        .with(logger.clone());
    tracing::subscriber::with_default(subscriber, || {
        emit();
        logger.flush().unwrap();
        let path = dir.path().join("app.log");
        let first = std::fs::read_to_string(&path).unwrap();
        assert!(first.contains("info event"));
        assert!(!first.contains("debug event"));
        reload.reload(EnvFilter::new("debug")).unwrap();
        emit();
        logger.flush().unwrap();
        let debug = std::fs::read_to_string(&path).unwrap();
        assert!(debug.contains("debug event"));
        assert!(debug.contains("[redacted]"));
        assert!(!debug.contains("private-value"));
        reload.reload(EnvFilter::new("off")).unwrap();
        let bytes = std::fs::metadata(&path).unwrap().len();
        emit();
        logger.flush().unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().len(), bytes);
        logger.clear().unwrap();
        reload.reload(EnvFilter::new("info")).unwrap();
        tracing::info!(target: "logging_test", "after clear");
        logger.flush().unwrap();
        let cleared = std::fs::read_to_string(&path).unwrap();
        assert!(cleared.contains("after clear"));
        assert!(!cleared.contains("info event"));
    });
    drop(worker);
}
#[test]
fn graceful_shutdown_drains_queued_events() {
    let dir = tempfile::tempdir().unwrap();
    let (logger, worker) = Logger::start(dir.path(), 1_000_000).unwrap().unwrap();
    tracing::subscriber::with_default(tracing_subscriber::registry().with(logger), || {
        for n in 0..50 {
            tracing::info!(target: "logging_test", n, "queued");
        }
    });
    drop(worker);
    let text = std::fs::read_to_string(dir.path().join("app.log")).unwrap();
    assert_eq!(text.lines().count(), 50);
    assert!(text.contains("n=49"));
}
