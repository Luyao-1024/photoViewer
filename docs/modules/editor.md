# Editor Module

## Scope

The editor module covers non-destructive operation state, orientation-metadata rotation with undo, crop/brightness/contrast/saturation controls, and save behavior.

## Key Files

| File | Role |
|---|---|
| `src/core/edit/mod.rs` | Edit state and registry surface |
| `src/core/edit/op.rs` | `EditOperation` trait |
| `src/core/edit/rotate.rs` | Non-destructive rotation operation |
| `src/core/edit/destructive_rotate.rs` | Orientation-metadata rotate plus undo path |
| `src/core/orientation.rs` | Read/write orientation metadata and apply it to display pixbufs |
| `src/core/edit/brightness.rs` | Brightness operation |
| `src/core/edit/contrast.rs` | Contrast operation |
| `src/core/edit/saturation.rs` | Saturation operation |
| `src/core/edit/crop.rs` | Crop operation |
| `src/core/edit/save.rs` | Save/apply behavior |
| `src/ui/editor_panel.rs` | Editor controls |
| `data/ui/editor-panel.blp` | Editor panel template |
| `tests/edit_ops.rs` | Operation coverage |
| `tests/e2e_editor.rs` | Editor flow coverage |

## Operation Pipeline

`EditOperation` defines `apply(&DynamicImage, ParamValue) -> DynamicImage`. `EditRegistry::new_with_v1` registers the built-in operations.

The pipeline order is rotation, brightness, contrast, saturation, then crop. Preserve this order unless a test and product decision explicitly require changing resulting image semantics.

## Editing Session

Entering the editor loads the source image into memory with orientation applied for display. Rotate, brightness, contrast, saturation, and crop controls update only `EditState` plus the preview texture. They must not mutate the source file, source metadata, database row, thumbnail cache, or create source backups before the user chooses a save action.

Save Copy writes a new file next to the source named `{stem}_edited_{milliseconds}.{ext}`. The timestamp is Unix epoch milliseconds, and the original extension is preserved. If the source stem already ends with `_edited_<digits>`, Save Copy replaces only that final timestamp suffix instead of appending another `edited` segment. Save Copy and Save Overwrite render edited pixels from the in-memory editing state, then save with the target path's image format so `.png` paths contain PNG bytes.

Orientation-metadata rotation is still implemented in `src/core/orientation.rs` and `src/core/edit/destructive_rotate.rs` for non-editor flows. The editor must treat rotation as a normal pending edit until Save Copy or Save Overwrite.

The editor footer exposes Save Copy as the suggested action and Save Overwrite as a direct danger-styled button. Save Overwrite still shows its confirmation dialog before writing.

## Adding An Operation

1. Implement `EditOperation`.
2. Register it in `EditRegistry`.
3. Add focused operation tests.
4. Wire UI controls only after the core operation is covered.
5. Update this module doc if the pipeline or save contract changes.
