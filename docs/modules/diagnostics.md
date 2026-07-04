# Diagnostics Module

## Scope

In-app crash and session diagnostics, so a crash leaves a usable trail in the
app's own files — not just whatever systemd/abrt happen to grab. Covers the
tracing file layer, the Rust panic hook, the GLib/GTK/GStreamer log redirect,
the native signal handler, and crash-file retention.

| File | Role |
|---|---|
| `src/core/diagnostics.rs` | `init()` (subscriber + file layer, panic hook, GLib redirect, retention), `write_panic_crash_log`, crash-file pruning |
| `src/core/crash_signal.rs` | Async-signal-safe `SIGSEGV`/`SIGABRT`/… handler that writes a crash file. The only `unsafe` in the feature. |
| `src/main.rs` | Calls `diagnostics::init()` as the **first** statement of `main()` |

## Output

All under `$XDG_CACHE_HOME/<app>/logs/`:

- `app.log` — current session trace (every `tracing` event plus redirected GLib messages). **Truncated each launch.**
- `crash-<unix_seconds>.log` — written by the panic hook (Rust panic) or the signal handler (native crash). Self-contained: a header + (panic: a `force_capture` backtrace) + the last 32 KiB of `app.log` as the lead-up.
- Retention: on every launch, only the **5 newest** `crash-*.log` are kept; older ones are deleted. `app.log` is never pruned by retention.

The launch log prints the resolved logs dir via `tracing::info!(target: "app", ...)`.

## Layer contract

1. **File + stderr subscriber.** `tracing_appender::non_blocking` layered under the existing `EnvFilter`, writing `app.log` (ANSI disabled). The worker guard is forgotten — the worker drains the channel for the process lifetime; crash paths flush their own files independently.
2. **Panic hook** writes `crash-<ts>.log` with payload, location, thread, `Backtrace::force_capture()`, and the `app.log` tail, then prints a short summary + the path to stderr.
3. **GLib log redirect** via `glib::log_set_writer_func`: every GObject/GTK/GStreamer message is re-emitted on tracing target `"glib"` (domain embedded in the message) and `Handled` is returned so GLib's own stderr writer doesn't duplicate it.
4. **Native signal handler** for `SIGSEGV`, `SIGABRT`, `SIGILL`, `SIGFPE`, `SIGBUS`, `SIGTRAP`, installed with `SA_SIGINFO | SA_ONSTACK` plus `sigaltstack` (so a stack-exhaustion `SIGSEGV` can still run the handler).

### Async-signal-safety (Layer 4)

The handler runs in async-signal context. It uses **only** POSIX async-signal-safe calls (`open`, `read`, `write`, `lseek`, `close`, `fsync`, `time`, `getpid`, `gettid` via `syscall`, `snprintf`, `sigaction`, `sigaltstack`, `raise`, `sigemptyset`) and reads only `&'static CStr` set once in `init` (no `format!`, no allocation, no `Mutex`). There is **no `backtrace()`** in the handler — it mallocs/dlopen and is not signal-safe. A re-entrancy `AtomicBool` guards against a fault *inside* the handler looping. After writing, the handler restores `SIG_DFL` and re-raises, so the process still terminates and systemd/abrt capture the core.

This is the **only** layer that can catch the native-crash class. A Rust panic hook cannot intercept a `SIGSEGV` raised inside a C library (libgobject/GstPlay) — which is the crash that motivated this module.

## Reading a crash file

The crash file header points at the real backtrace:

```
photo-viewer v0.9.0 caught signal 11 (SIGSEGV)
time: 1751000000
pid: 12345 tid: 12346
si_code: 1 fault_addr: 0xdead
note: full backtrace: coredumpctl info 12345  (or gdb on the saved core)

--- app.log tail (last 32 KiB) ---
…lead-up trace events…
```

For the stack, run `coredumpctl info <pid>` (or `coredumpctl gdb <pid>`) — the core carries the backtrace the signal handler deliberately does not capture. The `app.log` tail carries the *what was the app doing* context that a raw core lacks.

## Caveats

- The non-blocking tracing writer means the very last events before a crash may not be flushed into `app.log`. The panic hook captures its own backtrace independently, and the signal handler writes its own header, so crash files stay useful with a slightly stale tail.
- Logs live under the cache dir, which the Flatpak manifest already grants write access to — no extra portal/permission needed.
- `log_set_writer_func` can be set only once per process; `init()` is called exactly once from `main()`.
