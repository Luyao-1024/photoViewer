use photo_viewer::core::refresh::RefreshCoordinator;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

#[test]
fn album_refresh_is_single_flight_with_pending_replay() {
    let calls = Arc::new(AtomicUsize::new(0));
    let calls_for_job = calls.clone();
    let coordinator = RefreshCoordinator::new_for_tests(move || {
        calls_for_job.fetch_add(1, Ordering::SeqCst);
        Ok(())
    });

    assert!(coordinator.mark_albums_dirty());
    assert!(!coordinator.mark_albums_dirty());
    coordinator.finish_album_refresh_for_tests().unwrap();

    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[test]
fn startup_album_dirty_events_coalesce_while_refresh_runs() {
    let calls = Arc::new(AtomicUsize::new(0));
    let calls_for_job = calls.clone();
    let coordinator = RefreshCoordinator::new_for_tests(move || {
        calls_for_job.fetch_add(1, Ordering::SeqCst);
        Ok(())
    });

    assert!(coordinator.mark_albums_dirty());
    assert!(!coordinator.mark_albums_dirty());
    assert!(!coordinator.mark_albums_dirty());
    coordinator.finish_album_refresh_for_tests().unwrap();

    assert_eq!(calls.load(Ordering::SeqCst), 2);
}
