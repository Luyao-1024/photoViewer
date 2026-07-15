use super::*;

fn touch(dir: &Path, name: &str) -> PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, b"x").unwrap();
    p
}

#[test]
fn crash_filename_timestamp_parsing() {
    assert_eq!(
        parse_crash_timestamp("crash-1751000000.log"),
        Some(1751000000)
    );
    assert_eq!(parse_crash_timestamp("crash-1.log"), Some(1));
    // Non-numeric timestamps and unrelated files are ignored.
    assert_eq!(parse_crash_timestamp("crash-x.log"), None);
    assert_eq!(parse_crash_timestamp("crash-12.txt"), None);
    assert_eq!(parse_crash_timestamp("app.log"), None);
    assert_eq!(parse_crash_timestamp("crash-1751000000"), None);
}

#[test]
fn glib_writer_filters_known_gtk_render_noise() {
    assert_eq!(
        glib_log_disposition(
            glib::LogLevel::Info,
            "Gtk",
            "snapshot symbolic icon as texture using mask"
        ),
        None,
        "high-frequency GTK icon snapshot info is render noise"
    );
    assert_eq!(
        glib_log_disposition(
            glib::LogLevel::Warning,
            "Gtk",
            "snapshot symbolic icon as texture using mask"
        ),
        Some(GlibLogDisposition::Warn),
        "warnings must still be retained even when the text matches a noisy info message"
    );
}

#[test]
fn glib_writer_keeps_low_severity_messages_out_of_default_info_logs() {
    assert_eq!(
        glib_log_disposition(glib::LogLevel::Info, "Gtk", "some gtk info"),
        Some(GlibLogDisposition::Debug)
    );
    assert_eq!(
        glib_log_disposition(glib::LogLevel::Debug, "GStreamer", "pipeline detail"),
        Some(GlibLogDisposition::Debug)
    );
    assert_eq!(
        glib_log_disposition(glib::LogLevel::Warning, "GStreamer", "decode failed"),
        Some(GlibLogDisposition::Warn)
    );
    assert_eq!(
        glib_log_disposition(glib::LogLevel::Critical, "Gtk", "critical failure"),
        Some(GlibLogDisposition::Error)
    );
}

#[test]
fn retention_keeps_five_newest_crash_files() {
    let dir = tempfile::tempdir().unwrap();
    // 10 timestamped crash files.
    for ts in 1..=10 {
        touch(dir.path(), &format!("crash-{ts}.log"));
    }
    // Decoys that must be left untouched.
    touch(dir.path(), "crash-x.log");
    let app_log = touch(dir.path(), APP_LOG_NAME);

    retain_crash_files(dir.path());

    let mut remaining: Vec<i64> = std::fs::read_dir(dir.path())
        .unwrap()
        .flatten()
        .filter_map(|e| parse_crash_timestamp(e.file_name().to_str()?))
        .collect();
    remaining.sort();
    // Newest 5 timestamps survive.
    assert_eq!(remaining, vec![6, 7, 8, 9, 10]);
    // Decoys are not crash-stamped, but app.log must still be on disk.
    assert!(app_log.exists(), "app.log must survive retention");
}

#[test]
fn write_panic_crash_log_writes_expected_sections() {
    let dir = tempfile::tempdir().unwrap();
    // Provide an app.log so the tail section is exercised.
    std::fs::write(dir.path().join(APP_LOG_NAME), b"lead-up event line\n").unwrap();

    let path = write_panic_crash_log(
        dir.path(),
        "diag-probe panic",
        "src/probe.rs:42:7",
        "<diag-thread>",
        "  frame 0\n  frame 1",
    );

    let content = std::fs::read_to_string(&path).unwrap();
    let path_str = path.to_string_lossy().into_owned();
    let dir_str = dir.path().to_string_lossy().into_owned();
    assert!(
        path_str.starts_with(&dir_str),
        "crash file must live in logs dir"
    );
    assert!(
        path_str.contains("crash-"),
        "crash file name must carry the prefix"
    );
    assert!(content.contains(&format!("{} ", config::APP_ID)));
    assert!(content.contains("panic"));
    assert!(content.contains("payload: diag-probe panic"));
    assert!(content.contains("location: src/probe.rs:42:7"));
    assert!(content.contains("thread: <diag-thread>"));
    assert!(content.contains("backtrace:"));
    assert!(content.contains("frame 0"));
    assert!(content.contains("--- app.log tail"));
    assert!(content.contains("lead-up event line"));
}

/// The Chrome trace layer must serialize span/event field values. tracing-chrome
/// 0.7 defaults `include_args` to false, which silently drops every span field
/// (uri, queue_wait_ms, tier, media_id, cache_hit, …) — the trace then carries
/// only counts/durations, hiding the per-item latency breakdown. Guard the opt-in.
#[test]
fn chrome_layer_serializes_span_fields() {
    let src = include_str!("../diagnostics.rs");
    assert!(
        src.contains(".include_args(true)"),
        "ChromeLayerBuilder must call .include_args(true) so span fields are serialized into the trace"
    );
}

#[test]
fn chrome_layer_uses_flow_target_for_selective_chain_capture() {
    let src = include_str!("../diagnostics.rs");
    assert!(
        src.contains("telemetry::selective_trace_requested()"),
        "diagnostics must recognize a focused trace-chain selection"
    );
    assert!(
        src.contains("metadata.target() == log_targets::FLOW"),
        "focused capture must exclude unrelated process-wide spans"
    );
}
