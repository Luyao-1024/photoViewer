use super::*;
use chrono::TimeZone;
use std::io::Read;
fn entry(message: &str) -> Entry {
    Entry {
        time_ms: 1,
        day: "2026-10-16".into(),
        level: "INFO".into(),
        target: "test".into(),
        message: message.into(),
    }
}
fn day(n: u32) -> DateTime<Local> {
    Local
        .with_ymd_and_hms(2026, 10, n, 12, 0, 0)
        .single()
        .unwrap()
}
#[test]
fn launch_keeps_todays_runs_raw_and_compresses_only_earlier_days() {
    let dir = tempfile::tempdir().unwrap();
    let mut first = Store::open_at(dir.path(), 1_000_000, day(15))
        .unwrap()
        .unwrap();
    first.append(&entry("previous day")).unwrap();
    let path = first.path.clone();
    drop(first);
    let mut second = Store::open_at(
        dir.path(),
        1_000_000,
        day(15) + chrono::Duration::seconds(1),
    )
    .unwrap()
    .unwrap();
    assert!(path.exists());
    assert_ne!(second.path, path);
    second.append(&entry("second run")).unwrap();
    drop(second);
    let third = Store::open_at(dir.path(), 1_000_000, day(16))
        .unwrap()
        .unwrap();
    assert!(!path.exists());
    let archives: Vec<_> = fs::read_dir(dir.path().join("archive/2026-10-15"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(archives.len(), 2);
    let mut combined = String::new();
    for archive in archives {
        flate2::read::GzDecoder::new(File::open(archive).unwrap())
            .read_to_string(&mut combined)
            .unwrap();
    }
    assert!(combined.contains("previous day"));
    assert!(combined.contains("second run"));
    assert_eq!(third.file.metadata().unwrap().len(), 0);
}
#[test]
fn another_launcher_cannot_archive_a_live_run() {
    let dir = tempfile::tempdir().unwrap();
    let _store = Store::open_at(dir.path(), 1_000_000, day(15))
        .unwrap()
        .unwrap();
    assert!(Store::open_at(dir.path(), 1_000_000, day(16))
        .unwrap()
        .is_none());
    assert!(!dir.path().join("archive").exists());
}
#[test]
fn quota_bounds_long_runs_and_clear_keeps_the_writer_visible() {
    let dir = tempfile::tempdir().unwrap();
    for run in 0..4 {
        let mut store = Store::open_at(dir.path(), 1_000_000, day(15 + run))
            .unwrap()
            .unwrap();
        for n in 0..120 {
            let message: String = (0..160).map(|_| uuid::Uuid::new_v4().to_string()).collect();
            store
                .append(&entry(&format!("{run}:{n}:{message}")))
                .unwrap();
            assert!(size(dir.path()) <= 1_000_000);
        }
        if run == 3 {
            store.clear().unwrap();
            assert_eq!(size(dir.path()), 0);
            store.append(&entry("after cleanup")).unwrap();
            assert!(fs::read_to_string(dir.path().join("app.log"))
                .unwrap()
                .contains("after cleanup"));
            assert_eq!(size(dir.path()), store.file.metadata().unwrap().len());
        }
    }
}
#[test]
fn lowering_the_limit_evicts_old_segments_and_stays_writable() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(dir.path(), MAX_BYTES).unwrap().unwrap();
    for _ in 0..180 {
        store.append(&entry(&"x".repeat(16_000))).unwrap();
    }
    assert!(size(dir.path()) > 1_000_000);
    store.resize(1_000_000).unwrap();
    assert!(size(dir.path()) <= 1_000_000);
    store.append(&entry("still writable")).unwrap();
    assert!(fs::read_to_string(dir.path().join("app.log"))
        .unwrap()
        .contains("still writable"));
}
#[test]
fn retention_uses_log_dates_instead_of_the_archives_new_mtime() {
    let dir = tempfile::tempdir().unwrap();
    let today = dir.path().join("2026-10-16/today.log");
    let archived = dir.path().join("archive/2026-10-15/yesterday.log.gz");
    fs::create_dir_all(today.parent().unwrap()).unwrap();
    fs::create_dir_all(archived.parent().unwrap()).unwrap();
    fs::write(&today, vec![b'x'; 400_000]).unwrap();
    fs::write(&archived, vec![b'x'; 400_000]).unwrap();
    let _store = Store::open_at(dir.path(), 1_000_000, day(16))
        .unwrap()
        .unwrap();
    assert!(today.exists());
    assert!(!archived.exists());
    assert!(size(dir.path()) <= 1_000_000);
}
