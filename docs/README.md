# Documentation Index

This is the human entry point for project documentation. Agents should start from [`AGENTS.md`](../AGENTS.md), then open the matching module document before editing code.

## Source Of Truth

- Current behavior lives in [`modules/`](modules/), split by functional ownership.
- Cross-cutting development process lives in [`development.md`](development.md) and [`testing.md`](testing.md).
- Root files such as [`../README.md`](../README.md) and [`../CONTRIBUTING.md`](../CONTRIBUTING.md) stay short and link here instead of duplicating detailed commands.
- Historical specs and plans under [`superpowers/`](superpowers/) are archived context. Do not treat them as active behavior unless a current module document links to a specific invariant.
- The visual UI naming map under [`ui-naming-reference/`](ui-naming-reference/) is maintained documentation, not generated scratch output.

## Core Docs

| Document | Scope |
|---|---|
| [`architecture.md`](architecture.md) | Layer boundaries, runtime integration, navigation model, GTK widget pattern |
| [`development.md`](development.md) | Dependencies, build/run, Flatpak, visual checks, documentation workflow |
| [`testing.md`](testing.md) | Test commands, CI-equivalent policy, test layout, known GTK/GStreamer warnings |

## Functional Modules

| Module | Scope |
|---|---|
| [`modules/storage.md`](modules/storage.md) | SQLite, media model, preferences, scanner, filesystem watcher, thumbnails |
| [`modules/browsing.md`](modules/browsing.md) | Photos page, Year/Month/Day grouping, thumbnail grids, mode selector |
| [`modules/viewer.md`](modules/viewer.md) | Viewer page, image/video playback, overlay controls, thumbnail strip, details/editor panels |
| [`modules/albums-trash.md`](modules/albums-trash.md) | Albums, album detail, sidebar album management, trash restore/delete flows |
| [`modules/editor.md`](modules/editor.md) | Edit operation pipeline, editing session, save behavior |
| [`modules/keyboard.md`](modules/keyboard.md) | Shortcut normalization, scope selection, main-window routing, default keymap |
| [`modules/diagnostics.md`](modules/diagnostics.md) | Crash logs, panic hook, signal handler, flow tracing, performance workflow |
| [`modules/ui-design.md`](modules/ui-design.md) | Screen-by-screen UI design reference and shared interaction intent |
| [`modules/ui-liquid-glass.md`](modules/ui-liquid-glass.md) | Liquid Glass material system, CSS split, reusable classes, visual verification |

## Visual Reference

| Document | Scope |
|---|---|
| [`ui-naming-reference/index.html`](ui-naming-reference/index.html) | Visual UI component naming map for templates, widget IDs, drag/resize affordances |

Update this reference when adding, renaming, or removing a UI widget, changing a template `child-id`, or altering a drag/resize affordance.

## Historical Material

[`superpowers/specs/`](superpowers/specs/) and [`superpowers/plans/`](superpowers/plans/) preserve previous design and implementation planning. They are useful for rationale, but the maintained module docs above are the current source for behavior and workflow.
