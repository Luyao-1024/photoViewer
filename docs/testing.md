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

UX coverage is organized by user goal. Both UX binaries are built on one shared
harness, so "real input" means the same thing everywhere:

- **`tests/common/interaction.rs`** — `Ui`, a pointer and keyboard bound to one
  pick root. `click` / `right_click` / `double_click` hit-test first, then deliver
  press and release to the gesture controllers that control's click path runs
  through. `type_search` / `type_entry` / `activate_entry` for text, `press_key` /
  `key_gesture` for the production router, and `pump` / `wait_until` for the loop.
- **`tests/common/shell.rs`** — `Shell`, a presented `MainWindow` over a real
  SQLite library of real distinct JPEGs on a real filesystem, wired with the same
  pool, `ThumbnailLoader`, `media_list` and `DbActor` the app uses. `Shell::new()`
  seeds two photos; `Shell::with_photos(n)` more. `Shell::seed_extra_album()`
  adds a second folder album, and `Shell::seed_broken_photo()` adds a row whose
  bytes are not a decodable image — the fixture a media-error scenario needs,
  because the failure has to be in the file for the viewer's error path to be
  the thing under test. Every scenario drops its shell, which destroys the
  window, so runs do not starve each other's frame clock.

The primary deterministic gate is `tests/ux_click_flows.rs`. It initializes GTK
once and runs its scenarios serially, because multiple GTK application shells are
not safe to drive concurrently in one test process. Its journeys are complete
goals through the real sidebar, navigation stack, pages, persistence layer and
filesystem:

1. Search for a photo by name, open it, read its details, favorite it, brighten
   it, save a copy — then check the copy is a real file in the library, the
   original is byte-for-byte unchanged, and the copy's bytes differ.
2. Enter multi-select from a photo's context menu, copy the collection through the
   album picker, reopen the album by clicking its sidebar row, and open a copied
   photo in the Viewer.
3. Trash both photos through the confirmation dialog, verify the files left the
   library folder, cancel the selection, restore one and verify it came back on
   disk, then delete the other permanently.
4. Delete from the Viewer and use the toast's Undo — which must put the file back,
   not just re-flag the row.
5. Rename from the details panel — the real file changes name, the old path is
   gone, the library row follows, and the tile repaints under the new name.
6. Save Overwrite, confirming the destructive dialog — the original's bytes change,
   the `.bak` the dialog promised exists and holds the pre-edit bytes, and no extra
   library row appears.
7. Empty the whole Trash — rows gone, files gone for good, page still open.
8. Edit, ask to leave, be asked first — "keep editing" keeps the pending edit;
   "discard" closes the editor and writes nothing to disk.
9. Select through the context menu and favorite the selection from the batch bar.
10. Rotate a photo with all three rotation buttons — checking they compose as
    deltas and that CCW is its own path — then press Reset and check the editor
    returned to the state it opened in, disarmed itself, and wrote nothing to
    disk.
11. Enter crop mode, step the ratio selector with both arrows — check the
    selector is absent before the mode starts, that a 4:3 choice really narrows
    the staged rectangle, and that Reset clears the pending crop and leaves the
    mode.
12. Enter album multi-select with real albums ticked and press Cancel — check
    nothing was deleted, the pending selection was dropped, and Delete disarmed
    itself.
13. Delete one album through the batch bar and the confirmation dialog — check
    that album's photos reached the Trash rather than being destroyed, the album
    left the sidebar, and every other album's files are byte-for-byte unchanged.
14. Open a photo whose bytes cannot be decoded — the surface must name the file,
    Retry must re-run the load and land back on the same honest error, Show in
    File Manager must take a real press, and Escape must still get the user out.
15. Ignore one album and Delete another through the row's right-click menu — the
    contrast is the point: Ignore must leave every file on disk and only drop the
    rows, Delete must move the files to the Trash.
16. Drive the Viewer from the keyboard — `R`/`Shift+R` rotate and come back, `+`
    and `0` move the same zoom scale the toolbar does, arrows walk the same list
    the Next button walks, `E` lands in the same editor, and Escape still exits.

Plus interaction contracts for variants a journey would have to contort to reach:
double activation pushing exactly one Viewer, the mode capsule, keyboard routing
(including that a held key dispatches one action), the batch toolbar, Viewer
chrome and zoom state, sidebar navigation, album multi-select, the album picker's
copy and move, the album context menu, and the sync badge surviving the jump from a
grid tile to the Viewer header.

### Reaching a control that is below the fold

A control inside a `GtkScrolledWindow` — the editor's crop group is one — has no
pointer position until the user scrolls to it, so `Ui::click` correctly refuses it.
Scenarios use `Ui::scroll_to_reveal(&control, "label")` first, which moves the
nearest scrollable ancestor's vertical adjustment the way a wheel would and then
holds the control to the same reachability bar as any other. It is a precondition
of the interaction, in the same class as the shell claiming a desktop-sized
window: it moves the viewport and never the state under test (crop ratio, pending
edits and selection are untouched). GTK 4.22 exposes no way to fabricate a
`GdkEventScroll` from Rust and the bindings offer no `gtk_widget_scroll_to`, so the
scroll lands on the adjustment rather than through a synthesized event. A control
that scrolling genuinely cannot reveal still fails, with a message that says
"hidden or folded" rather than "off-screen".

Keep a contract only when putting the assertion into a journey would make the
journey branch unnaturally or hide the behavior being diagnosed. New UX
regressions should first extend the nearest journey; add an isolated widget test
only for a reusable widget contract or a state that cannot be reached
deterministically through GTK.

Run this focused gate when a change adds or modifies UX:

```bash
tools/with-at-spi.sh xvfb-run -a -s "-screen 0 1920x1080x24" cargo test --test ux_click_flows
```

### The display has to be desktop-sized

`xvfb-run`'s built-in default screen is **640x480**. A UX scenario asserts pointer
coordinates, and on a 640x480 display the window is clamped, `AdwBreakpointBin`
switches the layout to its narrow breakpoints, and controls that are perfectly
reachable for a user end up outside the surface. `Shell` therefore refuses to start
below 1000x700 and names the flag to pass. Use 1920x1080 for every UX command,
including CI.

## Pointer targeting: hit-test before you press

A `clicked` signal emitted with `emit_by_name` is not a click. It bypasses the
entire input path, so it cannot see anything that is wrong *between* the pointer
and the button: a transparent overlay, a filled `Gtk.Overlay` child, a stale
allocation. That gap is how the viewer's Previous/Next pair could stop responding
while the whole suite stayed green.

It hides the mirror-image problem too. Chrome a user cannot reach still answers an
emitted signal, so a test can "click" a button that was never revealed — and can
thereby paper over a state bug in the code that reveals it. Converting the suite to
real presses found exactly that: after a successful Trash restore or delete,
`finish_operation` handed an **empty** failure list to `grid.select_ids(&[])`, and
`select_ids` derives its multi-select flag from "is anything selected". The grid
dropped out of multi-select, so the remaining photos could no longer be ticked and
the batch bar never came back. The old test reached the same end state by calling
`select_ids` itself, which set the flag it was testing, so nothing was visible.

So a test that claims to cover a pointer affordance has to do what the input path
does, in this order:

1. let the widget be really laid out (a presented window and a mapped allocation,
   not a `compute_bounds` on an unrealized tree);
2. ask GTK where a pointer at that position lands — `gtk_widget_pick` — and assert
   it resolves to the control under test or one of its descendants;
3. hand press and release to the gesture controller(s) that control's click path
   runs through, in the controller's own coordinate space;
4. assert the user-visible result.

### Which widget owns the click

Measured against GTK 4.22 / libadwaita 1.6, not guessed:

| control | gesture owner |
|---|---|
| `Gtk.Button`, `ModeSelector` label cell, sidebar album row | the widget itself |
| `Gtk.GridView` / `Gtk.ListView` item | the `GtkListItemWidget` wrapper, **not** the tile |
| `Gtk.ListBox` row | the `GtkListBox` |
| `Gtk.FlowBox` child | the `Gtk.FlowBox` |
| `AdwActionRow` | its own gesture drives only its pressed visuals; `activated` comes from the enclosing `GtkListBox` resolving the press coordinates |
| `AdwAlertDialog` response | a real `Gtk.Button`, inside the dialog's **own surface** |

`Ui` therefore propagates a press innermost → outermost and stops at the first
owner that actually activates (`Ui::ends_the_line`: buttons, list boxes, list and
grid views, flow boxes, `GtkListItemWidget`, toggles, switches). Stopping matters as
much as continuing: delivering all the way to the window fires handlers a real press
never reaches, because the first controller claims the sequence.

Two consequences worth knowing before writing a case:

- `GtkListBoxRow` and `GtkFlowBoxChild` are **not** end-of-line. `AdwActionRow` is a
  `GtkListBoxRow` subclass with its own primary `GtkGestureClick`, and pressing only
  that gesture leaves `activated` unfired.
- `AdwAlertDialog` is its own surface: `compute_bounds()` from the main window
  returns `None` for its buttons, so content inside a dialog is driven through
  `Ui::for_widget(...)`, which walks to the top of the tree and binds to whichever
  surface really holds the widget.

### Waiting for the layout to settle

A hit test taken during a transition measures the wrong thing, and this shows up
constantly: a `GtkStack` crossfade keeps the outgoing page hit-testable, so the
first pointer after a page swap lands on the page the user already left.
`assert_reachable` allows a bounded settle window before failing, so it matches a
user who aims again, and still fails when a control never becomes reachable.

The same rule applies to virtualized grids. A realized tile is not a painted one and
the grid rebinds its item widgets when decoded thumbnails land, so a widget held
across a reload can point at a different photo — or at none. `Shell::tile_for`
resolves the tile **from the photo's identity** every time and waits for it to be
painted, and scenarios re-resolve after any action that reloads a grid.

### Aim at what the user reads

Scenarios name controls the way a user does: by visible label
(`find_button_with_label(&dialog, &tr("dialog.trash"))`), by displayed text
(`find_label_containing(&album_list, &album.display_name())`), or by which photo a
tile paints (`grid.tile_for_media(id)`). That is why an `Album` is aimed at by
`display_name()` — the sidebar row shows the folder's basename while `name` holds
the raw path.

Because those aims are *rendered strings*, they must come from `tr()`, never from a
literal. The process locale decides what a control says, and CI has no `LANG`, so
it gets the catalogue's English fallback while a developer's shell gets zh-CN. A
test that searched for a literal `日` on the mode capsule passed locally and failed
on CI with "no hittable Day cell" — the widget was laid out fine, the string it
paints is just `"Day"` there. `Photo mode` labels, dialog responses and page titles
all go through the catalogue; when a scenario needs one, it asks for
`tr("photo.mode.day")` and aims at whatever that returns.

### Text input

GTK 4.22 emits `GtkSearchEntry`'s `search-changed` only from its key-handler path:
`set_text`, `insert_text` and `activate` each leave a connected listener silent, and
Rust has no public API to fabricate the `GdkEvent` that path needs. So
`Ui::type_search` inserts each character through the editable's real insert path and
then emits the signal the keystroke would have emitted. This is the harness's one
measurable departure from event-level fidelity, and it is the reason a search
journey must not use `set_text`.

### Rules learned while writing the viewer suite

- Use the production entry point. `ViewerPage::new(media_list, index)` leaves
  `media_query` unset, so `prefetch_neighbors` returns early and the
  end-of-library arrow dimming — a real UX contract — never happens. Use
  `new_for_query`.
- A synthetic key press is a *gesture*, not a `key-pressed` event. The router
  latches a combo until a key release so auto-repeat cannot be dispatched as
  fresh presses, so a helper that emits `key-pressed` alone models a key held
  forever and every second press of that key is silently swallowed. Emit a
  matching `key-released`, and to test a *hold* deliver press + repeats +
  release as one unit — then assert on how many presses the router actually
  dispatched, which is deterministic, rather than on a visual end state that a
  stray pointer event could legitimately change.
- A widget that is not mapped has no pointer position. Its last allocation is
  stale and `compute_bounds` will happily return coordinates that belong to
  whatever sits there now, so a hit test taken on a folded or torn-down control
  measures the wrong thing. Return "nothing picked it" for unmapped widgets, and
  do not assert on animation-driven transitions.

`tests/ux_viewer_pointer_flows.rs` applies the harness to the Viewer: a real
`MainWindow`, a real `ViewerPage` pushed through `new_for_query` with a live
`MediaQuery`, and a library of real distinct JPEGs, across five cases — the
navigation pair walking the whole library to both ends, the stage centre never being
claimed by chrome, the zoom cluster, the header actions and the navigation lock, and
the chrome coming back clickable after an immersive fold.

```bash
tools/with-at-spi.sh xvfb-run -a -s "-screen 0 1920x1080x24" cargo test --test ux_viewer_pointer_flows
```

Because these suites need a display, keep a display-free source gate next to the
runtime one when a template invariant is what broke:
`tests/ui_viewer_source_structure.rs::overlay_chrome_revealers_are_not_fill_aligned`
parses the Revealer's own property lines (not the whole block — the wrapped child
has an alignment of its own and would otherwise satisfy the check) and runs under
plain `cargo test`.

The full-shell fixtures use valid media files and a normal filesystem under the
current user's home directory. This lets the journeys exercise image decode,
editor save, and GIO trash/restore behavior rather than pre-seeding their final
database states. Every fixture is isolated and cleaned up after the scenario.
A UX scenario must not call a page-level action handler, must not emit `clicked`,
and must not set a widget's end state (selection, visibility, a database row) to
reach the state under test — set it by interacting, then assert what the
interaction produced.

### CI renders an older GTK than this machine

CI installs `libgtk-4-dev` / `libadwaita-1-dev` from the Ubuntu runner image, which
is **GTK 4.14.5 + libadwaita 1.5.0**, while a desktop development machine is on
GTK 4.22.x. The application supports both — that is what
`grid_css::runtime_compatible_css` is for: GTK only learned `backdrop-filter` in
4.22, so on an older runtime the sheet actually installed is the authored sheet
with that one property removed.

Two rules follow, and both were learned from a red CI run rather than invented:

- A test that asserts GTK accepts a stylesheet must parse
  `runtime_compatible_css(&css_for_tests…())`, not the authored sheet. Asserting
  the authored one makes the test fail on 4.14 over a property that has nothing
  to do with what the test is about (`ui::motion::reduce_motion_block_parses_in_gtk_css`
  did exactly that). Tests that check the *authored* text — "the liquid block
  contains a blur" — are fine as they are; they never hand the CSS to GTK.
- A parse-clean or pass-on-this-machine result is not the same as pass-on-CI for
  anything that touches GTK internals. Gesture ownership and renderer behaviour
  are measured against the running version, so a GTK-dependent expectation has to
  be stated in terms of `gtk::check_version`, and the version CI runs belongs in
  this file because it is not visible from a green local suite.

### Environment and expected log noise

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

A real-pointer run also prints `Gdk-CRITICAL: gdk_event_get_modifier_state:
assertion 'GDK_IS_EVENT (event)' failed` a handful of times (five in the current
`ux_click_flows`). It is the cost of the harness: Rust emits `pressed`/`released`
on a `GtkGestureClick` without a `GdkEvent`, because GTK 4 exposes no public API
for fabricating one, and some handler partway up the press path then asks the
absent current event for its modifier state. Measured: no single harness
primitive produces it on its own — a left/right/double click, a key press, a hold
gesture, `type_search`, `type_entry` and a dialog response button are all silent
in isolation — and it appears only inside the scenarios that drive presses into
album lists and the album picker. The press still lands (the surrounding
expectations pass), so treat the line as noise, not as evidence of a skipped
click; if it ever becomes loud, that is a hint a new gesture owner is reading
the event. MESA `DRI3`/`vulkan` lines are the same story under `xvfb-run` —
software rendering, not a failure.

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
tools/with-at-spi.sh xvfb-run -a -s "-screen 0 1920x1080x24" cargo test --lib path::to::new_gtk_test
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
tools/with-at-spi.sh xvfb-run -a -s "-screen 0 1920x1080x24" cargo test --locked --all
```

The pushed commit's `Tests:` section must show the result of all four gate
commands. If a command fails because the base branch is already red, record the
command, failure, and evidence that it is unrelated; do not label the gate as
passing.

## Test Layers

- `tests/common/interaction.rs` + `tests/common/shell.rs`: the real-input harness
  and the full application shell every UX scenario drives. Shared by both UX
  binaries; new UX coverage starts here rather than reimplementing hit testing.
- `tests/ux_click_flows.rs`: full-shell user journeys first, then narrowly
  scoped interaction contracts. This is the deterministic UX release gate.
- `tools/visual-check-x11.sh`: Flatpak/runtime smoke checks and screenshots for
  behaviors the in-process GTK harness cannot reproduce faithfully.
- `tests/common/mod.rs`: shared test fixtures and helpers.
- `tests/fixtures/media/`: checked-in real media fixtures used by default
  tests, including phone HEIC/video coverage that must not be hidden behind
  `#[ignore]`.
- `tests/pipeline_*` (formerly named `e2e_*` / `e3e_*`): data-pipeline integration
  tests for scan/group, thumbnails, edit persistence, album and trash boundaries.
  They call `core::` functions directly, drive no GTK window, and are not UX
  end-to-end evidence; each file's header names the journey that covers its
  user-facing route.
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
