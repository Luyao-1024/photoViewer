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

For a Flatpak-runtime check of the non-destructive keyboard entry path, run `tools/visual-check-x11.sh --keyboard-smoke`. It sends `Ctrl+F` through XTEST on X11 and saves startup and Search-page screenshots; it complements the deterministic GTK suite rather than replacing it. Run `tools/visual-check-x11.sh --a11y-smoke` to additionally use `python3-pyatspi` against the live AT-SPI tree: it requires the localized application frame, a focused Search entry, and the three Search field toggle buttons exposed under their accessible names. Those names are resolved from `i18n/<locale>.json` for the locale the probe predicts the app picked (its own `--locale {zh-CN,en}` flag overrides, mirroring the app's config → `PHOTO_VIEWER_LOCALE` → `LC_ALL`/`LANG`/`LANGUAGE` precedence), so under zh-CN the expectation is `全部`、`文件名`、`日期` and under en it is `All`、`File name`、`Date`; `tests/visual_check_script.rs` pins the probe's zh-CN fallback list to the catalogue. This verifies the key Search navigation semantics available to assistive technology; it does not replace manual screen-reader usability testing.

### Probing the accessibility tree without Flatpak

When a change is about what a screen reader *sees* (roles, names, checked states),
the in-process GTK suite can only verify the half that GTK exposes readably -
`gtk_accessible_update_state`/`update_property` have no getter, so a test can read
`accessible_role()` but not the state it pushed. Read the live tree instead. The
application does not have to be a Flatpak for that:

```bash
mkdir -p /tmp/pv-a11y/home/Pictures && cp tests/fixtures/media/*.jpg /tmp/pv-a11y/home/Pictures/
env -u NO_AT_BRIDGE HOME=/tmp/pv-a11y/home PHOTO_VIEWER_LOCALE=zh-CN \
  tools/with-at-spi.sh xvfb-run -a ./target/debug/photo-viewer &
python3 - <<'PY'   # pip deps: python3-gi (Atspi 2.0)
import gi; gi.require_version("Atspi", "2.0")
from gi.repository import Atspi
Atspi.init()
d = Atspi.get_desktop(0)
app = next(d.get_child_at_index(i) for i in range(d.get_child_count())
           if (d.get_child_at_index(i).get_name() or "").lower().startswith("photo"))
def walk(n, depth=0):
    print(f"{'  ' * depth}{n.get_role_name()} :: {n.get_name()!r}")
    for i in range(n.get_child_count()):
        c = n.get_child_at_index(i)
        if c: walk(c, depth + 1)
walk(app)
PY
```

Three things will bite if you do not know them. `NO_AT_BRIDGE=1` is exported by
development shells here and silently disables GTK's AT-SPI bridge - the app then
appears on the bus with an empty tree, so unset it for the app, not just for the
probe. The probe joins whatever `DBUS_SESSION_BUS_ADDRESS` already points at, and
`/proc/<pid>/environ` of the *application process* (not the `xvfb-run` wrapper) is
the place to check. And GTK's ARIA roles are renamed on the way through AT-SPI:
`radio_group` shows up as `grouping`, `radio` as `radio button`, `button` as
`push button`, and `checked` is an `Atspi.StateType.CHECKED` flag rather than part
of the role. Reading `State::Checked` back this way is how P2-3 was confirmed:
`grouping '照片分组方式'` with three `radio button` children, one carrying
`CHECKED`. This probe is manual and needs a display, so it is not part of the CI
gate.

The same walk is what settled P2-4. A `GtkGridView` surfaces as `table` with
`table cell` children, so print the cells' names to check tile labels at all:
before the change every cell was `''`, and the first attempt - naming the
always-allocated selection checkmark - made the cells read `'已选中 已收藏'` while
still losing the file names. Two facts explain that and are worth remembering
before pushing a name anywhere: GTK ignores an accessible name on a plain
container role (the tile only became nameable once its class-level role was
`img`), and a decoration that is faded with CSS opacity rather than unparented is
still in the tree, so it must be `presentation`. To read a *state* rather than a
name, ask the node for its state set
(`node.get_state_set().contains(Atspi.StateType.SELECTED)`); nothing in the
in-process suite can, which is why "the checkmark is decoration, selection is not
yet an accessible state" is recorded as a limitation in
[`modules/browsing.md`](modules/browsing.md) instead of being asserted.

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
- `src/ui/grid_css/tests/render.rs`: contracts measured from real GTK rendering
  rather than from the CSS source — sampled pixel colours (the hover/selection
  scrim gap) and WCAG ratios computed from a foreground composited over the
  surface it was actually drawn on. Use this layer whenever a claim is about
  what a user sees, not about what the sheet says.
- Desktop-preference behavior is toggled, not mocked: `src/ui/motion/tests.rs`
  flips `GtkSettings:gtk-enable-animations` and restores it, then asserts the
  widget tree and the assembled sheet follow. `build_css*` variants keep a
  motion-on default so ordinary CSS tests stay independent of the machine
  running them.
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

### Measuring where a widget actually paints

When a test has to assert that two surfaces do not overlap, use
`Widget::compute_bounds(other)` and nothing else. `allocation()` is the widget's
own box *plus* its CSS margin, so an `AdwToast` reports 82px tall while its card
paints 46px, and a child of `Adw.ToastOverlay` can report an offset 32px away
from where it draws (`translate_coordinates` disagrees with the paint). Both
measurements looked like a real overlap that was not there, and the reverse
blind spot is worse: a CSS `margin-bottom` "lift" reads as movement in
`allocation()` while the card never moves. Pixel-level checks in
`src/ui/grid_css/tests/render.rs` sample the whole window for the same reason -
a single widget's snapshot omits its parent's background.
