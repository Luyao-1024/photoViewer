# Contributing

## Workflow

1. Create a focused branch.
2. Read the relevant module document from [docs/README.md](docs/README.md).
3. Add or update focused tests before implementation when the change affects behavior.
4. Run the smallest useful verification during development.
5. Before handoff, follow the CI policy in [docs/testing.md](docs/testing.md).
6. Update module documentation in the same change when contracts, UI invariants, or workflow change.

## Project Boundaries

- `src/core/`: database, scanner, metadata, thumbnails, albums, trash, preferences, edit pipeline
- `src/ui/`: GTK widgets, pages, templates, CSS providers, navigation wiring
- `src/platform/`: XDG and desktop integration
- `data/ui/`: Blueprint source templates; edit `.blp`, not generated `.ui`

For detailed architecture, build commands, testing expectations, and module ownership, use [docs/README.md](docs/README.md).
