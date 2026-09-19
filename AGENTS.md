# Agent Guide

This file is the entry point for coding agents working in this repository. Keep it short: detailed rules live in `docs/`.

## Start Here

1. Read the user's request and identify the affected functional module.
2. Open the matching module document under `docs/modules/` before editing code.
3. Inspect implementation with `rg`/targeted file reads; follow existing GTK/Rust patterns.
4. For behavior changes, add or update focused tests before implementation when practical.
5. While changes are uncommitted, run only the new or directly modified focused tests. Reserve the full CI suite for the remote-push gate.
6. Update the relevant module docs when changing contracts, UI invariants, or development workflow.

## Module Map

| Area | Read |
|---|---|
| Project layout and ownership | [`docs/architecture.md`](docs/architecture.md) |
| Build, run, Flatpak visual checks | [`docs/development.md`](docs/development.md) |
| Test strategy and known GTK warnings | [`docs/testing.md`](docs/testing.md) |
| Photos grid, grouping, mode selector | [`docs/modules/browsing.md`](docs/modules/browsing.md) |
| Full-screen viewer and viewer chrome | [`docs/modules/viewer.md`](docs/modules/viewer.md) |
| Albums and trash flows | [`docs/modules/albums-trash.md`](docs/modules/albums-trash.md) |
| Editor operations and save paths | [`docs/modules/editor.md`](docs/modules/editor.md) |
| Keyboard shortcut routing and keymap | [`docs/modules/keyboard.md`](docs/modules/keyboard.md) |
| DB, scanner, filesystem watcher, thumbnails, WebDAV sync | [`docs/modules/storage.md`](docs/modules/storage.md) |
| Crash logs, panic hook, signal handler | [`docs/modules/diagnostics.md`](docs/modules/diagnostics.md) |
| Screen-by-screen UI design reference | [`docs/modules/ui-design.md`](docs/modules/ui-design.md) |
| Visual UI component naming map | [`docs/ui-naming-reference/index.html`](docs/ui-naming-reference/index.html) |
| Liquid Glass and shared UI material classes | [`docs/modules/ui-liquid-glass.md`](docs/modules/ui-liquid-glass.md) |
| Historical milestone plans/specs | [`docs/superpowers/`](docs/superpowers/) |

## Development Rules

- Edit `data/ui/*.blp` templates, not generated `.ui` output. `build.rs` compiles Blueprint and bundles GResource assets.
- Keep `src/core/` independent from GTK UI concerns. UI widgets belong under `src/ui/`.
- Do not revert user changes or unrelated worktree changes.
- Prefer existing helpers and patterns over new abstractions.
- Keep docs and tests close to the module being changed.
- When you add/rename/remove a UI widget, change a template `child-id`, or alter a drag/resize affordance, update [`docs/ui-naming-reference/index.html`](docs/ui-naming-reference/index.html) to match. It is a maintained visual naming map (source of truth: `data/ui/*.blp`, `src/ui/*.rs`), not a one-time artifact.
- **Do not run the full suite merely because changes are still uncommitted.** During implementation, run only tests added or directly modified by the change (plus the narrowest command needed to execute them). A local commit may be created from that focused evidence.
- **Every commit message must contain a `Tests:` section** listing the commands actually run and their result (`PASS`, `FAIL`, or `NOT RUN` with a reason). Never report an unexecuted check as passing.
- **Run the full CI suite only before pushing commits to a remote.** Test the final code tree that will be pushed with exactly the commands from `.github/workflows/ci.yml`: `cargo fmt --all --check`, `cargo clippy --locked --all-targets -- -D warnings -A clippy::type_complexity -A clippy::too_many_arguments`, `cargo build --locked --all-targets`, and `tools/with-at-spi.sh xvfb-run -a cargo test --locked --all`. Record those results in the pushed commit's `Tests:` section; amend the message after the run when necessary. A message-only amend does not require rerunning the suite because the tested code tree is unchanged. A focused test is not a substitute at the push gate. If `main` is already red from an unrelated failure, record the exact failure instead of making the commit look green.

## UI Invariants

- The year/month/day mode selector is the canonical Liquid Glass segmented control. Preserve its visual model: one raised glass capsule, lightweight internal state, no per-segment active background block.
- Reuse the shared `.glass-*` and `.glass-segment*` CSS classes before adding selectors.
- Any new glass surface must work in both Liquid Glass and plain translucent modes.
- Viewer side panels must avoid hidden children participating in layout in a way that produces negative GTK allocation warnings.
- The main navigation sidebar keeps a stable width so pushing viewer pages does not shrink it.
- When a `GtkScrolledWindow` needs to size to content and scroll only when overflowing, use `propagate-natural-height: true` and omit `vexpand`. Default is `false`, which means the scrolled window reports 0 natural height to its parent — do not try to compensate with `vexpand` + `max-content-height` combos.

## Common Commands

```bash
cargo build
cargo run
cargo test
cargo test --test ui_grid_css_install
cargo fmt
cargo clippy --all-targets
./run-flatpak.sh
```

Run only at the remote-push gate — mirrors `.github/workflows/ci.yml`:

```bash
cargo fmt --all --check
cargo clippy --locked --all-targets -- -D warnings -A clippy::type_complexity -A clippy::too_many_arguments
cargo build --locked --all-targets
tools/with-at-spi.sh xvfb-run -a cargo test --locked --all   # GTK tests need a display and AT-SPI; matches CI
```
