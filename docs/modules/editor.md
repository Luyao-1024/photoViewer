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

Entering the editor loads the source image into memory with orientation applied for display. Rotate, brightness, contrast, saturation, crop, and reset controls update only `EditState` plus the preview texture. They must not mutate the source file, source metadata, database row, thumbnail cache, or create source backups before the user chooses a save action.

The reset button is a circular icon button in the editor header. It is enabled only when `EditState::has_pending_edits()` is true, and it restores rotation, adjustments, and crop to defaults without closing the editor.

Crop coordinates are stored in oriented source-image pixel coordinates so Save Copy and Save Overwrite apply the same crop to the original file. If the preview image is downsampled for rendering, `EditorPanel` scales the crop rectangle only for preview rendering. Crop controls stay inside the editor panel: Start Crop toggles crop mode, and the graphical ratio selector uses previous/next arrow buttons to switch original/1:1/4:3/3:2/16:9/free. The ratio preview is intentionally larger than a toolbar icon, while the previous/next buttons are narrow vertical controls. The crop completion button is a compact centered action, not a full-width row. The header reset button clears pending crop along with other unsaved edits.

While crop mode is active, the viewer keeps showing the full edited preview without applying the crop, then draws a draggable crop overlay above the image. Clicking or dragging the rectangle selects it and changes the border/handle treatment so users can see it is active. Dragging inside the rectangle moves it; dragging corner handles resizes it. The overlay writes changes back to `EditState.crop` without scheduling a preview render, so dragging the crop box only redraws the overlay and does not show the image loading spinner. The final crop is rendered when crop mode is finished or when the user saves.

Save Copy writes a new file next to the source named `{stem}_edited_{milliseconds}.{ext}`. The timestamp is Unix epoch milliseconds, and the original extension is preserved. If the source stem already ends with `_edited_<digits>`, Save Copy replaces only that final timestamp suffix instead of appending another `edited` segment. Save Copy and Save Overwrite render edited pixels from the in-memory editing state, then save with the target path's image format so `.png` paths contain PNG bytes.

Orientation-metadata rotation is still implemented in `src/core/orientation.rs` and `src/core/edit/destructive_rotate.rs` for non-editor flows. The editor must treat rotation as a normal pending edit until Save Copy or Save Overwrite.

The editor footer exposes Save Copy as the suggested action and Save Overwrite as a direct danger-styled button. Save Overwrite still shows its confirmation dialog before writing.

## Adding An Operation

1. Implement `EditOperation`.
2. Register it in `EditRegistry`.
3. Add focused operation tests.
4. Wire UI controls only after the core operation is covered.
5. Update this module doc if the pipeline or save contract changes.
