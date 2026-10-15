use super::*;
use crate::core::db::init_pool;

fn temp_pool(name: &str) -> crate::core::db::DbPool {
    let root =
        std::env::temp_dir().join(format!("photoviewer-device-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&root).expect("temp root");
    init_pool(&root.join("library.db")).expect("pool")
}

#[test]
fn a_device_id_is_persisted_so_consecutive_edits_look_like_one_device() {
    let pool = temp_pool("persist");
    let first = load_or_mint(&pool).unwrap();
    assert!(!first.is_empty());
    let stored: String = pool
        .get()
        .unwrap()
        .query_row(
            "SELECT value FROM settings WHERE key = ?1",
            [DEVICE_ID_KEY],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(stored, first);
    // A later load reuses the stored value rather than minting a second one:
    // two ids in one run would make one user's edits look like a race.
    assert_eq!(load_or_mint(&pool).unwrap(), first);
}

#[test]
fn separate_databases_have_stable_independent_device_identities() {
    // The process-wide identity is what every edit event in this run carries,
    // so it must be the same string however many times it is asked for.
    let pool = temp_pool("once");
    let first = device_id(&pool);
    assert!(!first.is_empty());
    assert_eq!(device_id(&pool), first);
    assert_ne!(device_id(&temp_pool("other-db")), first);
}

#[test]
fn an_edit_event_carries_a_real_time_a_source_and_a_fresh_id() {
    let stamp = crate::core::sync::store::EditEventStamp {
        edited_at_ms: now_millis(),
        source: device_id(&temp_pool("event")),
        event_id: new_event_id(),
    };
    // A real wall-clock instant, not a placeholder and not a file's mtime.
    assert!(
        stamp.edited_at_ms > 1_600_000_000_000,
        "{}",
        stamp.edited_at_ms
    );
    assert!(!stamp.source.is_empty());
    assert_ne!(
        stamp.event_id,
        new_event_id(),
        "each edit is distinguishable"
    );
}
