use super::format_capture_day;
use crate::core::i18n::tr;
use chrono::{Duration, Local, Utc};

#[test]
fn capture_day_none_when_absent() {
    assert_eq!(format_capture_day(None), None);
}

#[test]
fn capture_day_today_yesterday_older() {
    let now = Utc::now();
    let today_str = tr("viewer.date.today");
    let yesterday_str = tr("viewer.date.yesterday");

    // Subtracting exactly 24h always lands on the previous local calendar
    // day, regardless of timezone offset, so these comparisons are stable.
    assert_eq!(
        format_capture_day(Some(now)),
        Some(today_str.clone()),
        "current instant should render as today"
    );

    let yesterday = now - Duration::days(1);
    assert_eq!(
        format_capture_day(Some(yesterday)),
        Some(yesterday_str.clone()),
        "24h ago should render as yesterday"
    );

    let older = now - Duration::days(5);
    let older_str = format_capture_day(Some(older)).expect("older date should format");
    assert!(!older_str.is_empty());
    assert_ne!(older_str, today_str);
    assert_ne!(older_str, yesterday_str);
    // Day precision carries the correct year (covers year-boundary dates).
    let expected_year = older.with_timezone(&Local).format("%Y").to_string();
    assert!(
        older_str.contains(&expected_year),
        "older date {older_str:?} should contain year {expected_year:?}"
    );
}

#[test]
fn capture_day_is_day_precision_only() {
    // The header label must never leak an HH:MM:SS time component.
    let dt = Utc::now() - Duration::days(5);
    if let Some(s) = format_capture_day(Some(dt)) {
        assert!(
            !s.contains(':'),
            "date label must be day-precision, got {s:?}"
        );
    }
}
