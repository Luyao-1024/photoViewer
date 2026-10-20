//! A quiet period between a local change and the synchronization it implies.
//!
//! Importing a folder or starring fifty photos in a row produces one change per
//! file. Running a cycle for each of them would re-scan the library and take a
//! fresh upload lease over and over, all of it within the same second, so a
//! change does not start a run — it restarts a countdown, and only a countdown
//! that reaches its end starts one. A change that arrives while the countdown is
//! running pushes the end out again, so a burst of imports costs one run rather
//! than one run per file.
//!
//! This is the only background trigger. Idle time does not start a cycle.
//! Startup and a chain that reaches its cap each ask for one continuation of
//! work that is already owed — the former once per launch, the latter only
//! after a failure-free cycle made progress — so neither becomes a poll.
//! Failures do not arm a periodic retry.

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use tokio::sync::watch;

/// How long the library must stop changing before a run is worth starting.
///
/// Long enough that a folder drop or a favourite spree collapses into one run,
/// short enough that a single edit still feels immediate rather than deferred.
pub const QUIET_PERIOD: Duration = Duration::from_secs(10);

/// The moment of the most recent change.
///
/// `watch` rather than a plain flag so the wait can be woken directly: a polling
/// loop would either add latency or burn the CPU, and this has to react the
/// instant a change lands.
fn channel() -> &'static watch::Sender<Instant> {
    static CHANNEL: OnceLock<watch::Sender<Instant>> = OnceLock::new();
    CHANNEL.get_or_init(|| watch::Sender::new(Instant::now()))
}

/// Record that the library changed, restarting any countdown in progress.
///
/// Called from the single place a local change is written, and only once the
/// change has been attached to a job — a file nobody syncs, or a change made
/// while synchronization is switched off, must not schedule anything.
///
/// This fires inside the transaction that records the change, so a transaction
/// that later rolls back has still armed the countdown. That costs one run which
/// finds nothing to do; the alternative is a commit hook per mutation path, and
/// a spurious no-op run is the cheaper mistake.
pub fn note_local_change() {
    // `send_replace` rather than `send`: there may be no scheduler listening
    // yet, and a change made before one exists must not panic or be dropped from
    // the point of view of whoever starts it later.
    channel().send_replace(Instant::now());
}

/// Wait until the library has been quiet for `quiet`, then return.
///
/// Returns early only if the channel closes, which cannot happen while this
/// process holds the sender — so a caller can treat the return as "the library
/// settled".
pub async fn settled(quiet: Duration) {
    settled_from(channel().subscribe(), quiet).await
}

/// The countdown itself, over a caller-supplied channel.
///
/// Split out so tests can drive their own channel instead of the process-wide
/// one — a shared trigger would make every test depend on every other test.
pub(crate) async fn settled_from(mut changes: watch::Receiver<Instant>, quiet: Duration) {
    // The first change has to be awaited before the countdown means anything:
    // subscribing mid-burst reports the current value as already seen.
    if changes.changed().await.is_err() {
        return;
    }
    loop {
        tokio::select! {
            () = tokio::time::sleep(quiet) => return,
            changed = changes.changed() => {
                // Another change landed while counting: start the quiet period
                // over, so a burst ends up costing one run, not one per file.
                if changed.is_err() {
                    return;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
