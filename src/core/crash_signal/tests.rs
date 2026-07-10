use super::*;
use std::ffi::CString;

#[test]
fn signal_name_is_null_terminated_and_matches_known_signals() {
    for &sig in HANDLED_SIGNALS {
        let name = signal_name(sig);
        assert!(name.ends_with(b"\0"), "signal name must be NUL-terminated");
        assert!(
            name.len() > 1,
            "signal {sig} must have a real name, not bare ?"
        );
    }
    // Unknown signal falls back without panicking.
    assert_eq!(signal_name(999), b"?\0");
}

#[test]
fn format_fragments_are_null_terminated() {
    for f in [FMT_CRASH_PATH, FMT_APPLOG_PATH, FMT_HEADER] {
        assert!(f.ends_with(b"\0"), "printf format must be NUL-terminated");
    }
}

#[test]
fn cstr_helpers_round_trip() {
    let c = CString::new("/tmp/abc").unwrap();
    let s: &'static CStr = Box::leak(c.into_boxed_c_str());
    LOG_DIR.set(s).unwrap();
    assert_eq!(LOG_DIR.get().copied().unwrap().to_bytes(), b"/tmp/abc");
}
