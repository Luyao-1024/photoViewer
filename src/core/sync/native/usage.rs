//! Account storage usage, fetched only once a quota failure has to be explained.
//!
//! The overview row wants to say how much of the account's space is gone, but
//! nothing on a normal path needs those two numbers. Asking for them on a timer
//! would put a request in every cycle for a figure that is only ever read while
//! the account is over quota, so the reading is pulled at the moment it becomes
//! meaningful and then reused.

use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// The two figures the overview row shows. Both come from one response, so the
/// limit never has to be remembered separately from the usage against it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageUsage {
    pub quota_bytes: u64,
    pub used_bytes: u64,
}

/// How long a reading stays good enough to display.
///
/// An account that is over quota is not growing on its own — its uploads are
/// being refused — so the figure barely moves between attempts. The TTL exists
/// so a limit the user raised on the service is picked up, not so that every
/// failing cycle can re-ask.
const FRESH_FOR: Duration = Duration::from_secs(300);

#[derive(Default)]
struct Cache(Option<(Instant, StorageUsage)>);

fn cache() -> &'static Mutex<Cache> {
    static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(Cache::default()))
}

/// The last reading, if one has been taken and has not gone stale.
///
/// Reading is best-effort: a missing or expired reading leaves the overview row
/// showing the headline alone, which is still true, just less specific.
pub fn cached() -> Option<StorageUsage> {
    let guard = cache().lock().ok()?;
    let (taken, usage) = guard.0.as_ref()?;
    (taken.elapsed() < FRESH_FOR).then_some(*usage)
}

/// Remember a reading against the moment it was taken.
pub fn record(usage: StorageUsage) {
    if let Ok(mut guard) = cache().lock() {
        guard.0 = Some((Instant::now(), usage));
    }
}
