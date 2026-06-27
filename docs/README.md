# Documentation Index

Project documentation is organized by functional module. Use this page for human navigation; agents should start from [`AGENTS.md`](../AGENTS.md).

## Core Docs

| Document | Scope |
|---|---|
| [`architecture.md`](architecture.md) | Layer boundaries, runtime integration, GTK widget pattern |
| [`development.md`](development.md) | Build, run, Flatpak, Blueprint, documentation workflow |
| [`testing.md`](testing.md) | Test layout, commands, known GTK warnings |

## Functional Modules

| Module | Scope |
|---|---|
| [`modules/browsing.md`](modules/browsing.md) | Photos page, Year/Month/Day grouping, thumbnail grids, mode selector |
| [`modules/viewer.md`](modules/viewer.md) | Viewer page, overlay controls, thumbnail strip, details/editor panels |
| [`modules/albums-trash.md`](modules/albums-trash.md) | Albums, album detail, trash, restore/delete flows |
| [`modules/editor.md`](modules/editor.md) | Edit operation pipeline, destructive rotate, save behavior |
| [`modules/storage.md`](modules/storage.md) | SQLite, media model, scanner, watcher, thumbnails |
| [`modules/ui-design.md`](modules/ui-design.md) | Screen-by-screen UI design reference and shared interaction intent |
| [`modules/ui-liquid-glass.md`](modules/ui-liquid-glass.md) | Liquid Glass CSS split, reusable classes, visual verification |

## Historical Design Material

Milestone specs and implementation plans live in [`superpowers/`](superpowers/). Treat them as historical context unless a current module document points to them as an active invariant.
