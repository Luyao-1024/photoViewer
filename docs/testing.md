# Testing

## Commands

```bash
cargo test
cargo test --test <name>
cargo fmt
cargo clippy --all-targets
```

While changes are uncommitted, run only tests newly added or directly modified
for the change. Do not broaden to the full suite merely because the affected
code is shared; full-suite coverage belongs to the remote-push gate below.

## UX Test Strategy

UX coverage is organized by user goal. The primary deterministic gate is
`tests/ux_click_flows.rs`; it initializes GTK once and runs serially because
multiple GTK application shells are not safe to drive concurrently in one test
process. Its leading scenarios are complete journeys through the real
`MainWindow`, sidebar, navigation stack, pages, persistence layer, and
filesystem effects:

1. Search for a photo, open Viewer, inspect details, favorite it, edit it, and
   save a copy.
2. Select the Photos collection, copy it through the album picker, reopen the
   album from the sidebar, and open an item in Viewer.
3. Move selected Photos items to Trash through the confirmation dialog, then
   cancel selection, restore one item, and permanently delete the other.

The rest of that binary contains interaction contracts for important variants
such as rapid double activation, mode switching, keyboard routing, settings,
rename/zoom/rotate controls, and album multi-select. Keep a contract only when
putting the assertion into a journey would make the journey branch unnaturally
or hide the behavior being diagnosed. New UX regressions should first extend
the nearest journey; add an isolated widget test only for a reusable widget
contract or a state that cannot be reached deterministically through GTK.

Run this focused gate during development when the change adds or modifies the
UX journey:

```bash
tools/with-at-spi.sh xvfb-run -a cargo test --test ux_click_flows
```

The full-shell fixtures use valid media files and a normal filesystem under the
current user's home directory. This lets the journeys exercise image decode,
editor save, and GIO trash/restore behavior rather than pre-seeding their final
database states. Every fixture is isolated and cleaned up after the scenario.
The suite sends keyboard input through the production capture-phase router and
uses GTK click/activation/response signals; it must not call page-level action
handlers directly.

`tools/with-at-spi.sh` starts an isolated session D-Bus when needed, then
checks that both `org.a11y.Bus` and `org.a11y.atspi.Registry` are available
before GTK initializes. It makes missing accessibility infrastructure a test
failure instead of leaving an `AT-SPI bus` warning in the log. Run
`tools/with-at-spi.sh --check` to verify the environment without running a
test command. The Flatpak manifest and development runner also allow the
narrow `org.a11y.Bus` session-bus permission, so GTK inside the sandbox can
discover that accessibility bus.

After a successful wrapped command, the registry may print `A connection to
the bus can't be made` while the temporary session is shutting down. This is
post-test service cleanup, not an application startup connection failure; the
helper's readiness check remains the pass/fail signal.

For a Flatpak-runtime check of the non-destructive keyboard entry path, run `tools/visual-check-x11.sh --keyboard-smoke`. It sends `Ctrl+F` through XTEST on X11 and saves startup and Search-page screenshots; it complements the deterministic GTK suite rather than replacing it. Run `tools/visual-check-x11.sh --a11y-smoke` to additionally use `python3-pyatspi` against the live AT-SPI tree: it requires the localized application frame, a focused Search entry, and the `全部`、`文件名`、`日期` field toggle buttons. This verifies the key Search navigation semantics available to assistive technology; it does not replace manual screen-reader usability testing.

## Verification Stages

### Uncommitted Development

Run the narrowest command that executes every test added or directly modified
by the current change. Examples:

```bash
cargo test --lib path::to::new_test
cargo test --test changed_integration_test
tools/with-at-spi.sh xvfb-run -a cargo test --lib path::to::new_gtk_test
```

Do not run `cargo test --all` at this stage unless the change is immediately
being prepared for a remote push. Existing neighboring tests are optional
diagnostics, not a routine requirement. A local commit may be created after the
focused tests finish.

### Commit Message Test Record

Every commit message must contain a `Tests:` section. List the exact commands
that were run and record `PASS`, `FAIL`, or `NOT RUN` with a reason. Do not claim
success for a command that was not executed. For example:

```text
feat(viewer): preserve navigation state

Tests:
- PASS: cargo test --lib ui::viewer_page::tests::new_navigation_case
- NOT RUN: full CI (local development commit; not yet pushed)
```

Before pushing, update the commit message so the test record includes the full
gate result. If the code was committed before the full run, use a message-only
amend after the gate completes. The full run remains valid after that amend
because the commit tree did not change; any code change after the run invalidates
the record and requires rerunning the gate.

### Remote-Push Gate

Only before pushing commits to a remote, run the categories covered by CI for
the final code tree that will be pushed:

```bash
cargo fmt --all --check
cargo clippy --locked --all-targets -- -D warnings -A clippy::type_complexity -A clippy::too_many_arguments
cargo build --locked --all-targets
tools/with-at-spi.sh xvfb-run -a cargo test --locked --all
```

The pushed commit's `Tests:` section must show the result of all four gate
commands. If a command fails because the base branch is already red, record the
command, failure, and evidence that it is unrelated; do not label the gate as
passing.

## Test Layers

- `tests/ux_click_flows.rs`: full-shell user journeys first, then narrowly
  scoped interaction contracts. This is the deterministic UX release gate.
- `tools/visual-check-x11.sh`: Flatpak/runtime smoke checks and screenshots for
  behaviors the in-process GTK harness cannot reproduce faithfully.
- `tests/common/mod.rs`: shared test fixtures and helpers.
- `tests/fixtures/media/`: checked-in real media fixtures used by default
  tests, including phone HEIC/video coverage that must not be hidden behind
  `#[ignore]`.
- `tests/e2e_*` and `tests/e3e_*`: legacy-named data-pipeline integration tests.
  They validate scan/group, thumbnail, edit persistence, album, and trash
  boundaries, but do not drive the GTK UI and are not UX end-to-end evidence.
- `tests/ui_*`: GTK template, CSS, and widget behavior checks.
- `tests/*_flow.rs`: module-level behavior such as trash and destructive rotate.
- `src/**/tests.rs` and `src/**/tests/*.rs`: unit tests close to implementation.

Use the narrowest layer that can prove the risk, while keeping the user journey
as the owner of cross-page behavior. A save implementation edge case belongs in
the edit pipeline tests; the promise that a user can reach Save Copy from
Search and return to Viewer belongs in the UX journey.

## Test Ownership

Keep unit tests with the module that owns the behavior, but do not define
inline `mod tests { ... }` blocks inside production source files. Source files
should declare `#[cfg(test)] mod tests;`, with test bodies in child test files.

- Single-file modules use a sibling `tests.rs` file, such as
  `src/core/runtime_config/tests.rs`.
- Nested modules use a child test module, such as
  `src/ui/viewer/filmstrip/tests.rs` or
  `src/ui/media_grid/loading/tests.rs`.
- Cross-module source-structure assertions stay under `tests/`, such as
  `tests/inline_test_ownership.rs`.

Prefer behavior, query-plan, widget-state, and compile-time boundary tests over
asserting incidental source strings. Source-structure tests are appropriate
only for ownership rules that Rust's visibility/type system cannot express;
do not use them to pin function names, formatting, or implementation order.

<!-- TODO(test): Add fault-injection integration coverage for edit save: a
database-commit failure after publishing an overwrite must restore the `.bak`,
and a competing save must be rejected without modifying either output. -->

<!-- TODO(test): Add filesystem fault-injection coverage for trash restore and
permanent delete, including cross-device moves and DB commit failure rollback. -->

<!-- TODO(test): Add a deterministic watcher stress test for a burst of
create/modify/rename/delete events, asserting final-path coalescing and bounded
DB command submission. -->

<!-- TODO(test): Add subprocess helper coverage using descendant processes to
verify timeout/output-limit cleanup kills the whole process group. -->

<!-- TODO(test): Add GTK interaction coverage for editor preview token
cancellation and save-control disabling while a background render is pending. -->

## Liquid Glass Warnings

Some host GTK versions print parser warnings for `backdrop-filter`. This is expected when the host runtime does not support that CSS property. The target visual runtime is Flatpak GNOME 50; do not remove `backdrop-filter` just to silence host parser warnings.

The accessibility CSS block uses GTK-supported `:focus-visible` rules. Do not
reintroduce unsupported `@media` feature queries or `@keyframes`. The material
color-resolution test covers both themes and transparency endpoints; see
[`modules/ui-liquid-glass.md`](modules/ui-liquid-glass.md) for optional screenshots.

## GstPlay Teardown In Tests

Do not let a deliberately invalid `GtkMediaFile` be finalized while its native
GstPlay worker can still be discovering the source. That race can emit
GObject criticals or abort the shared `--lib` test process after every Rust
assertion has passed. Ownership-only tests should use a source-less stream.
Tests that must exercise `show_at` with fake video files keep one narrowly
scoped test reference alive until process exit; they must still detach and
pause the stream through `stop_video_playback` first. Do not copy this
test-only retention into production code.

Production teardown retains a detached stream for one main-loop idle cycle so
GstPlay can finish its terminal signal before the last normal reference is
released. See [`modules/viewer.md`](modules/viewer.md).

## GTK Allocation Warnings

Warnings such as negative width or height allocation usually mean hidden chrome is still participating in layout, a fixed-size area is being over-constrained, or an overlay child is measured while collapsed. Fix the layout cause rather than filtering logs.

Viewer side panels and overlay controls should have stable dimensions and should hide child content when collapsed if that content would otherwise force invalid allocation.
