//! Best-effort native signal handler that writes a `crash-<ts>.log` on fatal
//! signals (`SIGSEGV`, `SIGABRT`, `SIGILL`, `SIGFPE`, `SIGBUS`, `SIGTRAP`).
//!
//! This is the **only** layer that can catch the native-crash class — a Rust
//! panic hook cannot intercept a `SIGSEGV` raised inside a C library such as
//! libgobject/GstPlay, which is exactly the crash that motivated this module.
//!
//! ## Async-signal safety
//!
//! The handler runs in an async-signal-context: an unknown thread state, with
//! the thread probably holding allocator/collection locks. It therefore uses
//! **only** POSIX async-signal-safe primitives — `open`, `read`, `write`,
//! `lseek`, `close`, `fsync`, `time`, `getpid`, `gettid` (via `syscall`),
//! `snprintf`, `sigaction`, `sigaltstack`, `raise`, `sigemptyset` — and reads
//! only `&'static CStr` data set once during `install`. There is deliberately
//! **no `backtrace()`** in the handler (it mallocs/dlopen → not signal-safe);
//! the full stack is recovered from the systemd/abrt core via
//! `coredumpctl info <pid>`, which the crash file points at.
//!
//! The handler is strictly additive: if anything it does fails, it still
//! restores `SIG_DFL` and re-raises the signal, so the process dies exactly as
//! it would have without this module and systemd/abrt still capture the core.

use std::ffi::CStr;
use std::sync::OnceLock;

use anyhow::{Context, Result};

use crate::core::log_targets;

/// Bytes of `app.log` copied into the crash file as the lead-up. Must match
/// `diagnostics::APP_LOG_TAIL_BYTES`.
const APP_LOG_TAIL_BYTES: i64 = 32 * 1024;

/// `&'static CStr` inputs set once in [`install`], read by the handler.
static LOG_DIR: OnceLock<&'static CStr> = OnceLock::new();
static APP_ID: OnceLock<&'static CStr> = OnceLock::new();
static VERSION: OnceLock<&'static CStr> = OnceLock::new();
/// Re-entrancy guard so a fault *inside* the handler doesn't loop forever.
static HANDLING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

const HANDLED_SIGNALS: &[libc::c_int] = &[
    libc::SIGSEGV,
    libc::SIGABRT,
    libc::SIGILL,
    libc::SIGFPE,
    libc::SIGBUS,
    libc::SIGTRAP,
];

/// `printf`-style format fragments, each a NUL-terminated C string.
const FMT_CRASH_PATH: &[u8] = b"%s/crash-%ld.log\0";
const FMT_APPLOG_PATH: &[u8] = b"%s/app.log\0";
const FMT_HEADER: &[u8] = b"%s %s caught signal %d (%s)\ntime: %ld\npid: %d tid: %ld\nsi_code: %d fault_addr: 0x%lx\nnote: full backtrace: coredumpctl info %d  (or gdb on the saved core)\n\0";
const TAIL_HEADER: &[u8] = b"\n--- app.log tail (last 32 KiB) ---\n";

/// Install handlers for all [`HANDLED_SIGNALS`]. Stores the `&'static CStr`
/// inputs so the handler can read them without touching the heap.
pub fn install(
    log_dir: &'static CStr,
    app_id: &'static CStr,
    version: &'static CStr,
) -> Result<()> {
    let _ = LOG_DIR.set(log_dir);
    let _ = APP_ID.set(app_id);
    let _ = VERSION.set(version);

    install_altstack().context("sigaltstack for crash handler")?;

    for &sig in HANDLED_SIGNALS {
        // SAFETY: zeroed sigaction is valid; SA_SIGINFO gives us si_code/si_addr,
        // SA_ONSTACK runs the handler on the alternate stack so a stack-exhaustion
        // SIGSEGV can still be reported. We install for one signal at a time.
        let mut sa: libc::sigaction = unsafe { std::mem::zeroed() };
        sa.sa_flags = libc::SA_SIGINFO | libc::SA_ONSTACK;
        unsafe { libc::sigemptyset(&mut sa.sa_mask) };
        sa.sa_sigaction = handler as *const () as usize;
        let r = unsafe { libc::sigaction(sig, &sa, std::ptr::null_mut()) };
        if r != 0 {
            return Err(anyhow::anyhow!("sigaction for signal {sig} returned {r}"));
        }
    }
    tracing::info!(
        target: log_targets::APP,
        "diagnostics: native signal handler installed for {} fatal signals",
        HANDLED_SIGNALS.len()
    );
    Ok(())
}

/// mmap an alternate signal stack and install it via `sigaltstack`.
fn install_altstack() -> Result<()> {
    // SIGSTKSZ may be a runtime value on modern glibc; evaluate it at runtime
    // and floor at 64 KiB so deep-stack crashes still have room.
    let size = (libc::SIGSTKSZ).max(64 * 1024);
    // SAFETY: anonymous, private, RW mapping. Never unmapped — the stack must
    // outlive the process so it is available whenever a signal fires.
    let ptr = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            size,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
            -1,
            0,
        )
    };
    if ptr == libc::MAP_FAILED {
        return Err(anyhow::anyhow!("mmap for altstack failed"));
    }
    let stack = libc::stack_t {
        ss_sp: ptr,
        ss_flags: 0,
        ss_size: size,
    };
    // SAFETY: installing a valid altstack.
    let r = unsafe { libc::sigaltstack(&stack, std::ptr::null_mut()) };
    if r != 0 {
        return Err(anyhow::anyhow!("sigaltstack returned {r}"));
    }
    Ok(())
}

/// The signal handler. See the module docs for the async-signal-safety contract.
extern "C" fn handler(sig: libc::c_int, info: *mut libc::siginfo_t, _ctx: *mut libc::c_void) {
    // Re-entrancy: if we faulted while already inside this handler, bail to the
    // default disposition and re-raise rather than looping.
    if HANDLING
        .swap(true, std::sync::atomic::Ordering::SeqCst)
    {
        unsafe { reset_default_and_raise(sig) };
        return;
    }

    // SAFETY: signal-handler context; write_crash_file uses only
    // async-signal-safe primitives and &'static CStr state.
    unsafe { write_crash_file(sig, info) };

    // Restore default disposition and re-raise so the process terminates and
    // systemd/abrt capture the core (the real backtrace lives there).
    unsafe { reset_default_and_raise(sig) };
}

/// Write the self-contained `crash-<ts>.log`: a header (signal/pid/tid/…) plus
/// a best-effort tail of `app.log`. All async-signal-safe.
///
/// # Safety
/// Must only be called from a signal handler (or equivalent async-signal
/// context): it uses only async-signal-safe libc primitives and reads
/// `&'static CStr` state.
unsafe fn write_crash_file(sig: libc::c_int, info: *mut libc::siginfo_t) {
    // SAFETY: this whole fn only calls async-signal-safe libc functions and
    // reads &'static CStr state; the caller is a signal handler.
    unsafe {
        let log_dir = LOG_DIR.get().copied().unwrap_or(c"/tmp");
        let app_id = APP_ID.get().copied().unwrap_or(c"photo-viewer");
        let version = VERSION.get().copied().unwrap_or(c"v?");

        // crash-<ts>.log path.
        let mut path_buf = [0u8; 512];
        let mut t: libc::time_t = 0;
        libc::time(&mut t);
        libc::snprintf(
            path_buf.as_mut_ptr() as *mut libc::c_char,
            path_buf.len(),
            FMT_CRASH_PATH.as_ptr() as *const libc::c_char,
            log_dir.as_ptr(),
            t as libc::c_long,
        );

        let fd = libc::open(
            path_buf.as_ptr() as *const libc::c_char,
            libc::O_WRONLY | libc::O_CREAT | libc::O_TRUNC,
            0o600_i32 as libc::c_uint,
        );
        if fd < 0 {
            return;
        }

        // Header.
        let mut hdr = [0u8; 640];
        let sicode = (*info).si_code as libc::c_int;
        let fault = (*info).si_addr() as libc::c_ulong;
        let pid = libc::getpid();
        let tid = libc::syscall(libc::SYS_gettid) as libc::c_long;
        let written = libc::snprintf(
            hdr.as_mut_ptr() as *mut libc::c_char,
            hdr.len(),
            FMT_HEADER.as_ptr() as *const libc::c_char,
            app_id.as_ptr(),
            version.as_ptr(),
            sig,
            signal_name(sig).as_ptr() as *const libc::c_char,
            t as libc::c_long,
            pid,
            tid,
            sicode,
            fault,
            pid,
        );
        if written > 0 {
            let n = (written as usize).min(hdr.len() - 1);
            libc::write(fd, hdr.as_ptr() as *const libc::c_void, n);
        }

        append_app_log_tail(fd, log_dir);

        libc::fsync(fd);
        libc::close(fd);
    }
}

/// Open `<log_dir>/app.log`, seek to its last ~32 KiB, and stream it into the
/// crash file. Async-signal-safe (`open`/`lseek`/`read`/`write`/`close`).
unsafe fn append_app_log_tail(fd: libc::c_int, log_dir: &'static CStr) {
    unsafe {
        let mut app_path = [0u8; 512];
        libc::snprintf(
            app_path.as_mut_ptr() as *mut libc::c_char,
            app_path.len(),
            FMT_APPLOG_PATH.as_ptr() as *const libc::c_char,
            log_dir.as_ptr(),
        );
        let afd = libc::open(app_path.as_ptr() as *const libc::c_char, libc::O_RDONLY, 0);
        if afd < 0 {
            return;
        }
        libc::write(fd, TAIL_HEADER.as_ptr() as *const libc::c_void, TAIL_HEADER.len());

        let len = libc::lseek(afd, 0, libc::SEEK_END);
        let from = if len > APP_LOG_TAIL_BYTES {
            len - APP_LOG_TAIL_BYTES
        } else {
            0
        };
        libc::lseek(afd, from, libc::SEEK_SET);

        let mut buf = [0u8; 4096];
        loop {
            let n = libc::read(afd, buf.as_mut_ptr() as *mut libc::c_void, buf.len());
            if n <= 0 {
                break;
            }
            libc::write(fd, buf.as_ptr() as *const libc::c_void, n as usize);
        }
        libc::close(afd);
    }
}

/// Reset `sig` to its default disposition and re-raise it so the process dies
/// and a core is produced. Async-signal-safe.
unsafe fn reset_default_and_raise(sig: libc::c_int) {
    unsafe {
        let mut sa: libc::sigaction = std::mem::zeroed();
        libc::sigemptyset(&mut sa.sa_mask);
        sa.sa_sigaction = libc::SIG_DFL;
        libc::sigaction(sig, &sa, std::ptr::null_mut());
        libc::raise(sig);
    }
}

/// NUL-terminated signal name for the header line.
fn signal_name(sig: libc::c_int) -> &'static [u8] {
    match sig {
        libc::SIGSEGV => b"SIGSEGV\0",
        libc::SIGABRT => b"SIGABRT\0",
        libc::SIGILL => b"SIGILL\0",
        libc::SIGFPE => b"SIGFPE\0",
        libc::SIGBUS => b"SIGBUS\0",
        libc::SIGTRAP => b"SIGTRAP\0",
        _ => b"?\0",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    #[test]
    fn signal_name_is_null_terminated_and_matches_known_signals() {
        for &sig in HANDLED_SIGNALS {
            let name = signal_name(sig);
            assert!(name.ends_with(b"\0"), "signal name must be NUL-terminated");
            assert!(name.len() > 1, "signal {sig} must have a real name, not bare ?");
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
}
