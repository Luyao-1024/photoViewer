# Albums And Trash Modules

## Scope

Albums expose folders as browsable collections. Trash integrates system trash behavior with restore and delete flows.

## Key Files

| File | Role |
|---|---|
| `src/core/albums.rs` | Album query/model helpers |
| `src/core/album_ops.rs` | Album operations |
| `src/core/trash.rs` | Trash operations |
| `src/ui/albums_page.rs` | Albums overview |
| `src/ui/album_detail_page.rs` | Album detail grid |
| `src/ui/trash_page.rs` | Trash UI and actions |
| `data/ui/albums-page.blp` | Albums template |
| `data/ui/album-detail-page.blp` | Album detail template |
| `data/ui/trash-page.blp` | Trash template |
| `tests/e3e_albums_trash.rs` | End-to-end albums/trash flow |
| `tests/trash_flow.rs` | Trash behavior |

## Albums

Albums are folder-derived rather than a separate user-authored collection model. Keep album covers and counts derived from media rows so scanner/database state remains the source of truth.

## Trash

Trash views must distinguish reversible trash state from permanent delete. Database state and filesystem state need to remain consistent across restore/delete operations.

When touching trash flows, verify:

- Moving an item to trash hides it from live photo queries.
- Restoring makes it visible again.
- Permanent delete removes the expected record/file state.
- Multi-select actions keep selection and empty states coherent.
