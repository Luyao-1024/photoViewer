# Editor Module

## Scope

The editor module covers non-destructive operation state, destructive rotation with undo, crop/brightness/contrast/saturation controls, and save behavior.

## Key Files

| File | Role |
|---|---|
| `src/core/edit/mod.rs` | Edit state and registry surface |
| `src/core/edit/op.rs` | `EditOperation` trait |
| `src/core/edit/rotate.rs` | Non-destructive rotation operation |
| `src/core/edit/destructive_rotate.rs` | Destructive rotate plus undo path |
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

## Destructive Rotate

Destructive rotation has a short undo toast path. Changes here should verify both the immediate file/database result and the undo behavior.

## Adding An Operation

1. Implement `EditOperation`.
2. Register it in `EditRegistry`.
3. Add focused operation tests.
4. Wire UI controls only after the core operation is covered.
5. Update this module doc if the pipeline or save contract changes.
