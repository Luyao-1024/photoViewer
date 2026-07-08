# Diagnostics Module

## Scope

In-app crash and session diagnostics, so a crash leaves a usable trail in the
app's own files — not just whatever systemd/abrt happen to grab. Covers the
tracing file layer, the Rust panic hook, the GLib/GTK/GStreamer log redirect,
the native signal handler, and crash-file retention.

| File | Role |
|---|---|
| `src/core/diagnostics.rs` | `init()` (subscriber + file layer, panic hook, GLib redirect, retention, optional chrome trace layer), `write_panic_crash_log`, crash-file pruning |
| `src/core/crash_signal.rs` | Async-signal-safe `SIGSEGV`/`SIGABRT`/… handler that writes a crash file. The only `unsafe` in the feature. |
| `src/main.rs` | Calls `diagnostics::init()` as the **first** statement of `main()` |

## Output

All under `$XDG_CACHE_HOME/<app>/logs/`:

- `app.log` — current session trace (every `tracing` event plus redirected GLib messages). **Truncated each launch.**
- `crash-<unix_seconds>.log` — written by the panic hook (Rust panic) or the signal handler (native crash). Self-contained: a header + (panic: a `force_capture` backtrace) + the last 32 KiB of `app.log` as the lead-up.
- `trace.json` — **only when `PHOTOVIEWER_CHROME_TRACE` is set.** A Chrome/Perfetto-format flow trace (see [Flow tracing](#flow-tracing-chromeperfetto)). Finalized on normal process exit; an unfinished/missing file means the process was killed/crashed mid-trace.
- Retention: on every launch, only the **5 newest** `crash-*.log` are kept; older ones are deleted. `app.log` (and `trace.json`) are never pruned by retention.

The launch log prints the resolved logs dir via `tracing::info!(target: "app", ...)`.

## Layer contract

1. **File + stderr subscriber.** `tracing_appender::non_blocking` layered under the existing `EnvFilter`, writing `app.log` (ANSI disabled). The worker guard is forgotten — the worker drains the channel for the process lifetime; crash paths flush their own files independently.
2. **Panic hook** writes `crash-<ts>.log` with payload, location, thread, `Backtrace::force_capture()`, and the `app.log` tail, then prints a short summary + the path to stderr.
3. **GLib log redirect** via `glib::log_set_writer_func`: GObject/GTK/GStreamer warnings and errors are re-emitted on tracing target `"glib"` (domain embedded in the message), while message/info/debug entries are downgraded to `debug` and high-frequency known render noise is dropped. `Handled` is returned so GLib's own stderr writer doesn't duplicate retained messages.
4. **Native signal handler** for `SIGSEGV`, `SIGABRT`, `SIGILL`, `SIGFPE`, `SIGBUS`, `SIGTRAP`, installed with `SA_SIGINFO | SA_ONSTACK` plus `sigaltstack` (so a stack-exhaustion `SIGSEGV` can still run the handler).
5. **Optional Chrome trace layer** (off unless `PHOTOVIEWER_CHROME_TRACE` is set) — see [Flow tracing](#flow-tracing-chromeperfetto). Not a crash-diagnostic layer; it captures per-flow timing for performance work.

### Async-signal-safety (Layer 4)

The handler runs in async-signal context. It uses **only** POSIX async-signal-safe calls (`open`, `read`, `write`, `lseek`, `close`, `fsync`, `time`, `getpid`, `gettid` via `syscall`, `snprintf`, `sigaction`, `sigaltstack`, `raise`, `sigemptyset`) and reads only `&'static CStr` set once in `init` (no `format!`, no allocation, no `Mutex`). There is **no `backtrace()`** in the handler — it mallocs/dlopen and is not signal-safe. A re-entrancy `AtomicBool` guards against a fault *inside* the handler looping. After writing, the handler restores `SIG_DFL` and re-raises, so the process still terminates and systemd/abrt capture the core.

This is the **only** layer that can catch the native-crash class. A Rust panic hook cannot intercept a `SIGSEGV` raised inside a C library (libgobject/GstPlay) — which is the crash that motivated this module.

## Flow tracing (Chrome/Perfetto)

The four layers above always run. A separate **opt-in** layer captures per-flow wall-clock timing for performance work, replacing the ad-hoc `elapsed_ms` log lines that used to be sprinkled through the hot paths.

**Enabling:** set `PHOTOVIEWER_CHROME_TRACE=1` (any non-falsey value: `0`/`false`/`off`/`no`/empty disable it). `init()` then attaches a `tracing_chrome::ChromeLayer` writing `<logs>/trace.json`; the `FlushGuard` is held in `main()` and finalizes the file on normal exit.

**Reading:** drop `trace.json` into <https://ui.perfetto.dev> (or `chrome://tracing`) for a timeline/flamechart view. The Chrome-trace JSON schema (`traceEvents` with `ts`/`dur`/`name`/`pid`/`tid`) is also directly parseable, so the same file serves both human inspection and AI analysis.

**Instrumented flows** (`#[tracing::instrument]`; the Perfetto span `name` is shown):

| Span name | Where |
|---|---|
| `scan:scan_and_aggregate`, `scan:notify_blocking` | `core/bootstrap.rs` startup scan |
| `thumb:generate` (nests `thumb:pb_decode`/`pb_scale`/`pb_save`), `thumb:process`, `thumb:set_prewarm_thumbnail_size` | `core/thumbnails.rs` per-image decode (decode/scale/save sub-phases), worker per-item envelope, and background-prewarm size changes |
| `photos:mode_selector_set_active`, `photos:mode_selector_stack_notify`, `photos:mode_stack_switch`, `photos:sync_active_grid_rebuilds`, `photos:mode_contrast_schedule`, `photos:mode_prewarm_size` | `ui/mode_selector.rs` and `ui/photos_page.rs` Year/Month/Day selector → stack notify → active-grid handoff → floating-selector contrast scheduling and thumbnail-prewarm size sync |
| `grid:load_metadata`, `grid:set_active`, `grid:rebuild_immediately`, `grid:scheduled_rebuild`, `grid:rebuild`, `grid:clear_content`, `grid:extract_items`, `grid:group_sections`, `grid:build_sections`, `grid:page_query` (+ `grid:db_page`), `grid:thumb_request`, `grid:reprioritize`, `grid:try_virtual_page`, `grid:expand_render_limit` | `ui/media_grid.rs` grid activation, rebuild trigger, coarse rebuild phases, virtual-scroll page load, per-tile thumb request, and the **fast-scroll hot path**: debounced visible-tile reprioritize (with a thumbnail `queue_len`/`in_flight` snapshot), virtual-page retarget (no synchronous rebuild — marks retargets only), and near-bottom render-limit expansion |
| `viewer:show_at`, `viewer:orig_decode`, `viewer:thumb_preview`, `viewer:navigate`, `viewer:nav_db_query` | `ui/viewer_page.rs` viewer switch, async decode/preview, navigation |
| `editor:save_as_copy`, `editor:save_overwrite` | `core/edit/save.rs` |
| `ui:apply_upserted_batch`, `ui:apply_startup_insertions` | `ui/apply_to_media_list.rs` shared list-store batch apply |
| `sidebar:rebuild_album_rows`, `sidebar:apply_album_rows`, `sidebar:apply_album_snapshot` | `ui/window.rs` sidebar album rows |
| `album:select_row`, `album:open_idle`, `album:open` (+ `album:already_visible_check`/`pop`/`load`/`store`/`page_build`/`bind_page`/`push`), `album:backfill_schedule`, `album:backfill_fetch` | `ui/window.rs` sidebar album selection → idle handoff → album open/page-build phases → background backfill |
| `album_detail:new` (+ `empty_state`/`grid_build`/`splice`), `album_detail:refresh_virtual`, `album_detail:filter_items` | `ui/album_detail_page.rs` album-detail page build + virtual refresh |

**Coverage caveat:** a span captures wall-clock of the function body on the calling thread. Work that escapes the function — a `spawn_blocking` DB query or an async decode that resolves *after* the caller returns — gets its own dedicated span entered in the async completion (e.g. `grid:db_page` inside the page-query worker, `viewer:orig_decode` for the original-image decode, `album:backfill_fetch`). Read end-to-end latency as the sequence of spans on the timeline.

For album-switch jank, launch with `PHOTOVIEWER_CHROME_TRACE=1`, reproduce several rapid album changes, close the app normally, then inspect `<cache-dir>/logs/trace.json` in Perfetto. Use the sequence `album:select_row` → `album:open_idle` → `album:load` → `album_detail:grid_build` → `album:push` to distinguish row-selection/idling, synchronous DB loading, grid construction, and navigation-push cost.

For **fast-scroll stalls on the Photos page** ("thumbnails freeze / stop loading while flinging"), launch with `PHOTOVIEWER_CHROME_TRACE=1` (debug build, or raise the release cap, so the new spans' fields are present), reproduce the fast scroll, close the app normally, then inspect `<cache-dir>/logs/trace.json` in Perfetto. The fast-scroll path used to be `debug!`-only and therefore invisible in a release trace; these INFO spans now cover it:

- `grid:reprioritize` — fires once per `grid_reprioritize_debounce_ms` tick. Its **duration** is the main-thread cost of the visible-tile viewport scan (`collect_visible_cache_keys` walks every FlowBox child + `compute_bounds`). Its fields disambiguate the stall class: `visible_keys` (0 = the post-rebuild scan found nothing to request), `queue_len` and `in_flight` (queue empty + stuck = requests not issued / workers idle-starved; `in_flight` high + stuck = workers saturated or blocked in generation).
- `grid:try_virtual_page` — fires only on a real page retarget (no-op guards are zero-cost). A retarget no longer does a synchronous skeleton rebuild (that was removed — it made the scrollbar jump up to the retarget ratio, then down to the landing, on every page swap); it just updates window state and spawns the DB query, so this span is now near-zero duration and marks retargets. `outcome` is `coalesced` (a DB page was already in flight) or `triggered`.
- `grid:expand_render_limit` — fires only on a real near-bottom expansion; chaining into `grid:rebuild` shows render-limit-driven rebuild storms near the list end.

**Fast-scroll "stuck/blank tiles" root cause: a widget-parenting bug in the tile-reuse path (pre-existing, now fixed).** Every real-tile rebuild rescued loaded tiles by MediaId (`detach_reusable_loaded_tiles`) and re-appended them, but the rescue only removed the `FlowBoxChild` from its `FlowBox` — the tile stayed parented to that `FlowBoxChild`. Under GTK toggle-ref finalization timing the `FlowBoxChild` was not finalized before the rebuild re-appended the tile, tripping `gtk_flow_box_child_set_child` and leaving the reused tile blank (set_child is skipped on the failed assertion). On overlapping virtual-page landings (adjacent windows share MediaIds) this produced blank/stuck tiles during scroll. The fix: `detach_reusable_loaded_tiles` explicitly `set_child(None)` to detach the tile from its `FlowBoxChild`, and the build loop defensively skips any rescued tile that is still parented (building it fresh instead). The landing rebuild itself is a plain immediate `rebuild_immediately` (full page) — a deferred and a progressive landing were both tried and reverted: deferral broke scroll-position restoration (jumped to top), and progressive fill destroyed-and-rebuilt tiles faster than thumbnails could load (a rebuild storm that re-triggered this assertion).

**The per-landing freeze is addressed by moving thumbnail disk reads off the main thread.** `build_photo_picture` used to call `try_load_cached` synchronously for every tile, so building a ~500-tile page did ~500 main-thread thumbnail reads + pixbuf decodes — the bulk of the ~1s freeze. It now uses `try_load_mem_cached` (in-memory LRU only, O(1), no disk I/O): a tile paints instantly only when its thumbnail is already resident (recently viewed); the rest stay on the `thumb-loading` placeholder and load via the viewport scan → async worker, whose `generate()` consults the disk cache off the main thread. So thumbnails arrive a few ms later instead of blocking the frame, and the rebuild cost drops to widget construction only.

Read end-to-end: if `thumb:process` spans keep completing but tiles do not paint, the main thread is saturated by long/dense `grid:reprioritize` / `grid:rebuild` spans (mechanism A). If `thumb:process` spans are sparse/long and `in_flight` stays high, workers are the bottleneck (mechanism B). If `visible_keys` drops to 0 after a rebuild or `enqueue_failed`/tile-drop warnings appear, requests were dropped or never re-issued (mechanism C).

**Release gating:** `tracing` is built with `release_max_level_info`, so in release builds `#[instrument]` spans plus `info!`/`warn!`/`error!` events are compiled in (flow timing available on demand), while `debug!`/`trace!` events compile out (zero overhead, zero log noise). The Chrome layer itself attaches only when the env var is set, so a normal release run pays nothing extra. To capture `debug!`-level detail in a trace, rebuild with `release_max_level_debug` (or trace in a debug build) and raise `RUST_LOG`, e.g. `RUST_LOG=photo_viewer=trace`.

**Complementary sampling profilers** (for CPU hotspots you did not instrument): `samply record ./target/release/photo-viewer` → <https://profiler.firefox.com>; or GNOME's `sysprof` for GSK/GLib-aware capture including render frames.

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
