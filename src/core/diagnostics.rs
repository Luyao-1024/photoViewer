//! Crash and session diagnostics.
//!
//! Four layers, all wired in [`init`], which `main()` runs as its very first
//! statement (before GTK, resources, or the DB) so panics and logs from every
//! later stage are captured:
//!
//! 1. A non-blocking file writer layered under the existing `EnvFilter`
//!    subscriber, writing `<log_dir>/app.log` (truncated each launch).
//! 2. A Rust panic hook that writes a self-contained `crash-<ts>.log`.
//! 3. A GLib/GTK/GStreamer log writer redirect, so every `GObject-CRITICAL`,
//!    GTK warning, and GStreamer error lands in `app.log` (and in crash files,
//!    via the tail).
//! 4. A best-effort native signal handler (see [`crate::core::crash_signal`])
//!    that writes a `crash-<ts>.log` on `SIGSEGV`/`SIGABRT`/… — the only layer
//!    that can catch the native-crash class (a Rust panic hook cannot).
//!
//! Crash files older than the 5 most recent are pruned on launch.

use std::ffi::CString;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::Local;
use tracing_subscriber::EnvFilter;

use crate::config;
use crate::core::log_targets;

const CRASH_FILE_PREFIX: &str = "crash-";
const CRASH_FILE_SUFFIX: &str = ".log";
const KEEP_CRASH_FILES: usize = 5;
const APP_LOG_NAME: &str = "app.log";
/// Bytes of `app.log` copied into a crash file as the lead-up.
const APP_LOG_TAIL_BYTES: i64 = 32 * 1024;

/// Install all four diagnostics layers. Must be the first call in `main()`.
///
/// Returns an optional Chrome trace `FlushGuard` when the
/// `PHOTOVIEWER_CHROME_TRACE` env var is set. `main()` must hold this binding
/// for the whole process lifetime: the trace file (`<log_dir>/trace.json`) is
/// only finalized (closing bracket written) when the guard is dropped on normal
/// exit. A crashed/`SIGKILL`ed process will leave an unfinished trace — that is
/// expected; crashes are diagnosed via the crash-log layers, not the trace.
pub fn init() -> Result<Option<tracing_chrome::FlushGuard>> {
    let log_dir = make_log_dir()?;

    // Start each session with an empty `app.log`. The previous session's
    // lead-up is preserved inside whatever `crash-<ts>.log` was written at its
    // end, so truncating here loses nothing.
    let _ = std::fs::remove_file(log_dir.join(APP_LOG_NAME));

    // Layer 1 first, so the hook/redirect layers below can log their own setup.
    // The returned guard is the optional Chrome trace flush handle (see fn doc).
    let chrome_flush_guard = install_subscriber(&log_dir);
    install_panic_hook(log_dir.clone());
    install_glib_log_redirect();
    install_signal_handler(&log_dir);

    // Prune before we (potentially) add a new crash file this session.
    retain_crash_files(&log_dir);

    tracing::info!(
        target: log_targets::APP,
        "diagnostics: session log -> {}/{}  (crash files: {}<ts>{})",
        log_dir.display(),
        APP_LOG_NAME,
        CRASH_FILE_PREFIX,
        CRASH_FILE_SUFFIX
    );
    if chrome_flush_guard.is_some() {
        tracing::info!(
            target: log_targets::APP,
            "diagnostics: chrome trace layer enabled -> {}/trace.json  \
             (raise detail with RUST_LOG=photo_viewer=trace)",
            log_dir.display()
        );
    }
    Ok(chrome_flush_guard)
}

fn make_log_dir() -> Result<PathBuf> {
    let dir = config::cache_dir().join("logs");
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("creating diagnostics dir {}", dir.display()))?;
    Ok(dir)
}

// ---- Layer 1: subscriber + file writer -------------------------------------

fn install_subscriber(log_dir: &Path) -> Option<tracing_chrome::FlushGuard> {
    use tracing_subscriber::fmt;
    use tracing_subscriber::prelude::*;

    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    let stderr_layer = fmt::layer().with_writer(std::io::stderr);

    let file_appender = tracing_appender::rolling::never(log_dir, APP_LOG_NAME);
    let (file_writer, guard) = tracing_appender::non_blocking(file_appender);
    // The guard's only job is graceful shutdown flush, which a crashing process
    // never reaches. Forgetting it lets the worker thread drain the channel for
    // the whole process lifetime; the panic/signal layers flush their own files.
    std::mem::forget(guard);
    let file_layer = fmt::layer().with_ansi(false).with_writer(file_writer);

    // Optional Layer 5: Chrome/Perfetto trace layer. Off by default; enabled only
    // when PHOTOVIEWER_CHROME_TRACE is set (see `chrome_trace_requested`), so
    // release builds pay no extra overhead unless a flow trace is requested. The
    // default EnvFilter ("info") already admits every `#[instrument]` span
    // (info-level), so flow timing is captured out of the box; raise RUST_LOG
    // (e.g. photo_viewer=trace) for finer detail. The FlushGuard is returned to
    // `main()` so the trace finalizes on normal exit (see `init` doc).
    let (chrome_layer, chrome_guard) = match chrome_trace_requested() {
        true => {
            let path = log_dir.join("trace.json");
            match std::fs::File::create(&path) {
                Ok(file) => {
                    let (layer, flush_guard) = tracing_chrome::ChromeLayerBuilder::new()
                        .writer(file)
                        .build();
                    (Some(layer), Some(flush_guard))
                }
                // Rare (logs dir is already writable); fall back to no chrome layer.
                Err(_) => (None, None),
            }
        }
        false => (None, None),
    };

    tracing_subscriber::registry()
        .with(env_filter)
        .with(stderr_layer)
        .with(file_layer)
        .with(chrome_layer)
        .init();

    chrome_guard
}

/// Whether the optional Chrome trace layer should be enabled this session.
/// Treated as an opt-in flag: any value other than the obvious falsey forms
/// (`0`, `false`, `off`, `no`, empty) enables it; unset disables it.
fn chrome_trace_requested() -> bool {
    match std::env::var("PHOTOVIEWER_CHROME_TRACE") {
        Ok(v) => {
            let v = v.trim();
            !v.is_empty()
                && !v.eq_ignore_ascii_case("0")
                && !v.eq_ignore_ascii_case("false")
                && !v.eq_ignore_ascii_case("off")
                && !v.eq_ignore_ascii_case("no")
        }
        Err(_) => false,
    }
}

// ---- Layer 2: panic hook ---------------------------------------------------

fn install_panic_hook(log_dir: PathBuf) {
    std::panic::set_hook(Box::new(move |info| {
        // Best-effort throughout: the hook itself must never panic, or we abort
        // with a double-panic and lose the report.
        let payload = panic_payload(info);
        let location = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "<unknown location>".to_string());
        let thread = std::thread::current();
        let thread_name = thread.name().unwrap_or("<unnamed>").to_string();
        let backtrace = std::backtrace::Backtrace::force_capture().to_string();

        let path = write_panic_crash_log(&log_dir, &payload, &location, &thread_name, &backtrace);

        eprintln!(
            "\n===== PANIC =====\n{payload}\nat {location}\nthread: {thread_name}\ncrash log: {}\n=================",
            path.display()
        );
    }));
}

fn panic_payload(info: &std::panic::PanicHookInfo) -> String {
    if let Some(s) = info.payload().downcast_ref::<&str>() {
        return (*s).to_string();
    }
    if let Some(s) = info.payload().downcast_ref::<String>() {
        return s.clone();
    }
    "<non-string panic payload>".to_string()
}

/// Write a self-contained `crash-<ts>.log` for a Rust panic. Safe to call from
/// the panic hook (not async-signal-constrained): uses `format!`, `std::fs`,
/// and chrono freely. Best-effort — io errors are logged to stderr, not raised.
fn write_panic_crash_log(
    log_dir: &Path,
    payload: &str,
    location: &str,
    thread: &str,
    backtrace: &str,
) -> PathBuf {
    let ts = unix_now();
    let path = log_dir.join(format!("{CRASH_FILE_PREFIX}{ts}{CRASH_FILE_SUFFIX}"));
    let result = (|| -> std::io::Result<()> {
        let mut f = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&path)?;
        writeln!(f, "{} {} panic", config::APP_ID, env!("CARGO_PKG_VERSION"))?;
        writeln!(f, "time: {} (unix {ts})", Local::now().format("%+"))?;
        writeln!(f, "thread: {thread}")?;
        writeln!(f, "location: {location}")?;
        writeln!(f, "payload: {payload}")?;
        writeln!(f)?;
        writeln!(f, "backtrace:")?;
        writeln!(f, "{backtrace}")?;
        append_app_log_tail(&mut f, log_dir)?;
        Ok(())
    })();
    if let Err(e) = result {
        eprintln!(
            "diagnostics: failed to write panic crash log {}: {e}",
            path.display()
        );
    }
    path
}

/// Append the last `APP_LOG_TAIL_BYTES` of `app.log` to a crash file so the
/// report is self-contained: the diagnostics context (what the app was doing)
/// travels with the crash, not just the stack.
fn append_app_log_tail(f: &mut File, log_dir: &Path) -> std::io::Result<()> {
    writeln!(f)?;
    writeln!(
        f,
        "--- app.log tail (last {} KiB) ---",
        APP_LOG_TAIL_BYTES / 1024
    )?;
    let mut src = match File::open(log_dir.join(APP_LOG_NAME)) {
        Ok(s) => s,
        Err(_) => return Ok(()), // no session log yet
    };
    let len = src.metadata().map(|m| m.len() as i64).unwrap_or(0);
    let from = if len > APP_LOG_TAIL_BYTES {
        len - APP_LOG_TAIL_BYTES
    } else {
        0
    };
    let _ = src.seek(SeekFrom::Start(from as u64));
    let mut buf = vec![0u8; 4096];
    loop {
        let n = src.read(&mut buf)?;
        if n == 0 {
            break;
        }
        f.write_all(&buf[..n])?;
    }
    Ok(())
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// ---- Layer 3: GLib/GTK/GStreamer log redirect ------------------------------

fn install_glib_log_redirect() {
    // Replaces GLib's global log writer. All GLib/GTK/GStreamer messages now
    // flow through tracing → app.log + crash tails. Returning `Handled`
    // suppresses GLib's own stderr writer so there's no duplication.
    glib::log_set_writer_func(glib_writer);
}

fn glib_writer(level: glib::LogLevel, fields: &[glib::LogField]) -> glib::LogWriterOutput {
    glib_log_to_tracing(level, fields);
    glib::LogWriterOutput::Handled
}

fn glib_log_to_tracing(level: glib::LogLevel, fields: &[glib::LogField]) {
    let mut domain = "glib";
    let mut message = "";
    for field in fields {
        match field.key() {
            "GLIB_DOMAIN" | "G_LOG_DOMAIN" => {
                if let Some(v) = field.value_str() {
                    domain = v;
                }
            }
            "MESSAGE" => {
                if let Some(v) = field.value_str() {
                    message = v;
                }
            }
            _ => {}
        }
    }
    // tracing's `target:` field wants a literal; embed the domain in the
    // message and emit on a fixed target. tracing writes via stdio/file, never
    // GLib, so there is no redirect recursion.
    let line = format!("[{domain}] {message}");
    match level {
        glib::LogLevel::Error | glib::LogLevel::Critical => {
            tracing::error!(target: "glib", "{line}")
        }
        glib::LogLevel::Warning => tracing::warn!(target: "glib", "{line}"),
        glib::LogLevel::Message | glib::LogLevel::Info | glib::LogLevel::Debug => {
            tracing::info!(target: "glib", "{line}")
        }
    }
}

// ---- Layer 4: native signal handler wiring ---------------------------------

fn install_signal_handler(log_dir: &Path) {
    let log_dir_cstr = leak_path_cstr(log_dir);
    let app_id = leak_str_cstr(config::APP_ID);
    let version = leak_str_cstr(&format!("v{}", env!("CARGO_PKG_VERSION")));
    // SAFETY: installs process-wide signal handlers. The handler reads only
    // the &'static CStr values below and uses async-signal-safe primitives.
    // See `crash_signal::install` for the full safety argument.
    if let Err(e) = crate::core::crash_signal::install(log_dir_cstr, app_id, version) {
        tracing::warn!(
            target: log_targets::APP,
            "diagnostics: native signal handler not installed ({e})"
        );
    }
}

fn leak_str_cstr(s: &str) -> &'static std::ffi::CStr {
    let c = CString::new(s).expect("diagnostics: NUL in static cstr literal");
    Box::leak(c.into_boxed_c_str())
}

fn leak_path_cstr(path: &Path) -> &'static std::ffi::CStr {
    use std::os::unix::ffi::OsStrExt;
    let c = CString::new(path.as_os_str().as_bytes())
        .expect("diagnostics: log path must not contain NUL");
    Box::leak(c.into_boxed_c_str())
}

// ---- Crash-file retention --------------------------------------------------

fn retain_crash_files(log_dir: &Path) {
    let entries = match std::fs::read_dir(log_dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    let mut stamped: Vec<(i64, PathBuf)> = Vec::new();
    for entry in entries.flatten() {
        let file_name = entry.file_name();
        let Some(name) = file_name.to_str() else {
            continue;
        };
        if let Some(ts) = parse_crash_timestamp(name) {
            stamped.push((ts, entry.path()));
        }
    }
    // Newest first; keep the top N, delete the rest.
    stamped.sort_by_key(|b| std::cmp::Reverse(b.0));
    for (_, path) in stamped.iter().skip(KEEP_CRASH_FILES) {
        let _ = std::fs::remove_file(path);
    }
}

fn parse_crash_timestamp(name: &str) -> Option<i64> {
    let core = name
        .strip_prefix(CRASH_FILE_PREFIX)?
        .strip_suffix(CRASH_FILE_SUFFIX)?;
    core.parse::<i64>().ok()
}

#[cfg(test)]
mod tests {
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
}
