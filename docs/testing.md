# Testing

## Commands

```bash
cargo test
cargo test --test <name>
cargo fmt
cargo clippy --all-targets
```

Use focused integration tests during development, then broaden when touching shared UI/CSS, storage, navigation, or edit behavior.

Run `cargo test --test ux_click_flows` before pushing/uploading a branch with UI interaction changes. Local edits and commits do not require this gate, but upstream handoff does.

## Pre-Submission CI Policy

Before pushing or otherwise handing off a change, make sure the same categories
covered by CI have run for the exact commit being handed off:

```bash
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings -A clippy::type_complexity -A clippy::too_many_arguments
cargo build --all-targets
xvfb-run -a cargo test --all
```

If GitHub Actions has already run these checks successfully for the exact
commit, use that CI result as the verification record instead of rerunning the
same full local commands. Do not duplicate expensive local test runs when the
remote CI result already covers the change. Run extra local commands only when
they cover something CI does not, such as a narrower reproduction, an
environment-specific Flatpak visual check, or a manual debugging path.

## Test Layout

- `tests/common/mod.rs`: shared test fixtures and helpers.
- `tests/fixtures/media/`: checked-in real media fixtures used by default
  tests, including phone HEIC/video coverage that must not be hidden behind
  `#[ignore]`.
- `tests/e2e_*`: user-flow level coverage.
- `tests/ux_*`: GTK signal-level UX flows that simulate user clicks/activations.
- `tests/ui_*`: GTK template, CSS, and widget behavior checks.
- `tests/*_flow.rs`: module-level behavior such as trash and destructive rotate.
- `src/**` unit tests: small invariants close to implementation.

## Liquid Glass Warnings

Some host GTK versions print parser warnings for `backdrop-filter`. This is expected when the host runtime does not support that CSS property. The target visual runtime is Flatpak GNOME 50; do not remove `backdrop-filter` just to silence host parser warnings.

The accessibility CSS block is intentionally empty unless implemented through GTK-supported settings or runtime classes. Do not reintroduce unsupported `@media` feature queries or `@keyframes`.

## GTK Allocation Warnings

Warnings such as negative width or height allocation usually mean hidden chrome is still participating in layout, a fixed-size area is being over-constrained, or an overlay child is measured while collapsed. Fix the layout cause rather than filtering logs.

Viewer side panels and overlay controls should have stable dimensions and should hide child content when collapsed if that content would otherwise force invalid allocation.
