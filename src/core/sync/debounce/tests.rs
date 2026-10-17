use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use super::*;

/// Spawn the countdown on its own channel and hand back a flag that flips when
/// it considers the library quiet.
///
/// Every test drives its own channel: the real trigger is process-wide, so
/// sharing it would make each test depend on every other one running alongside.
fn countdown(quiet: Duration) -> (watch::Sender<Instant>, Arc<AtomicBool>) {
    let (sender, receiver) = watch::channel(Instant::now());
    let settled = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&settled);
    tokio::spawn(async move {
        settled_from(receiver, quiet).await;
        flag.store(true, Ordering::SeqCst);
    });
    (sender, settled)
}

/// Record a change and let the countdown observe it before time moves.
///
/// The order matters: a change the task has not seen yet does not start its
/// quiet period, so advancing the clock first would start that period late and
/// the run would land later than the ten seconds the user was promised.
async fn change(sender: &watch::Sender<Instant>) {
    sender.send_replace(Instant::now());
    tokio::task::yield_now().await;
    tokio::task::yield_now().await;
}

/// Move the clock forward and then let the woken task actually run.
///
/// `advance` only moves the deadline; on a single-threaded runtime the task it
/// woke still needs a turn before its flag flips.
async fn pass(elapsed: Duration) {
    tokio::time::advance(elapsed).await;
    tokio::task::yield_now().await;
    tokio::task::yield_now().await;
}

fn has_settled(flag: &AtomicBool) -> bool {
    flag.load(Ordering::SeqCst)
}

#[tokio::test(start_paused = true)]
async fn an_idle_library_never_schedules_a_run() {
    let (_sender, settled) = countdown(QUIET_PERIOD);
    tokio::task::yield_now().await;
    pass(QUIET_PERIOD * 4).await;
    assert!(
        !has_settled(&settled),
        "with nothing changing there is nothing to send"
    );
}

#[tokio::test(start_paused = true)]
async fn a_single_change_waits_out_the_whole_quiet_period() {
    let (sender, settled) = countdown(QUIET_PERIOD);
    tokio::task::yield_now().await;
    change(&sender).await;

    pass(QUIET_PERIOD - Duration::from_millis(1)).await;
    assert!(
        !has_settled(&settled),
        "the run must not start the instant the file changes"
    );

    pass(Duration::from_millis(1)).await;
    assert!(has_settled(&settled), "a change left alone must be sent");
}

#[tokio::test(start_paused = true)]
async fn changes_during_the_quiet_period_push_the_run_out_again() {
    let (sender, settled) = countdown(QUIET_PERIOD);
    tokio::task::yield_now().await;
    change(&sender).await;

    // Five further changes, as a folder import would produce. Each restarts the
    // period, so the run lands ten seconds after the last change rather than
    // ten seconds after the first.
    for _ in 0..5 {
        pass(QUIET_PERIOD * 3 / 5).await;
        change(&sender).await;
        assert!(
            !has_settled(&settled),
            "a change must restart the countdown instead of letting it expire"
        );
    }

    pass(QUIET_PERIOD / 2).await;
    assert!(
        !has_settled(&settled),
        "seconds of continuous importing are not a quiet period"
    );
    pass(QUIET_PERIOD).await;
    assert!(
        has_settled(&settled),
        "once the library goes quiet the run is due"
    );
}

#[tokio::test(start_paused = true)]
async fn a_change_made_while_the_run_is_in_flight_is_not_lost() {
    // The scheduler is busy starting a cycle when a change lands. The next
    // countdown subscribes fresh and must still wait for its own change, rather
    // than reporting a library quiet the moment it starts looking.
    let (sender, first) = countdown(QUIET_PERIOD);
    tokio::task::yield_now().await;
    change(&sender).await;
    pass(QUIET_PERIOD * 2).await;
    assert!(has_settled(&first));

    let (_next_sender, next) = countdown(QUIET_PERIOD);
    tokio::task::yield_now().await;
    pass(QUIET_PERIOD * 2).await;
    assert!(
        !has_settled(&next),
        "a fresh countdown waits for its own change; it must not fire on its own"
    );
}

#[test]
fn the_quiet_period_is_ten_seconds() {
    assert_eq!(QUIET_PERIOD, Duration::from_secs(10));
}
