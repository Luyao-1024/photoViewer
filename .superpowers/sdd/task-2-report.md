# Task 2 Report

## 实现摘要

- 新建 `src/ui/viewer/crop.rs`，将 viewer crop overlay 的状态类型、绘制/拖拽方法以及纯几何辅助函数从 `src/ui/viewer_page.rs` 拆出。
- 在 `src/ui/viewer_page.rs` 中声明 `mod crop;`，保留 `CropDragState` 给 `imp::ViewerPage` 使用，并通过 `#[cfg(test)]` 导入保住现有 viewer 单测入口。
- 扩展 `tests/ui_viewer_source_structure.rs`，要求 `crop.rs` 存在且拥有 crop overlay 相关接口，同时约束这些 helper 不再留在 `viewer_page.rs`。
- 更新 `docs/modules/viewer.md`，把 `src/ui/viewer/crop.rs` 记入 viewer 模块职责表。

## TDD RED/GREEN 证据

### RED

命令：

```bash
cargo test --test ui_viewer_source_structure
```

结果：

- 失败。
- 关键报错：`src/ui/viewer/crop.rs should exist`

### GREEN

命令：

```bash
cargo fmt
cargo test --test ui_viewer_source_structure
cargo test ui::viewer_page
```

结果：

- `cargo fmt` 成功。
- `cargo test --test ui_viewer_source_structure` 通过，`1 passed; 0 failed`。
- `cargo test ui::viewer_page` 通过，`71 passed; 0 failed`。
- `ui::viewer_page` 运行中出现既有 GTK theme parser warnings（`backdrop-filter`），与本次 crop 模块迁移无关。

## 测试命令和结果

1. `cargo test --test ui_viewer_source_structure`
   - RED：失败，缺少 `src/ui/viewer/crop.rs`
2. `cargo fmt`
   - 通过
3. `cargo test --test ui_viewer_source_structure`
   - GREEN：通过
4. `cargo test ui::viewer_page`
   - GREEN：通过，71 个 viewer_page 相关单测全部通过

## 修改文件

- `src/ui/viewer_page.rs`
- `src/ui/viewer/crop.rs`
- `tests/ui_viewer_source_structure.rs`
- `docs/modules/viewer.md`

## 自检

- 只修改了 Task 2 指定文件。
- crop overlay 的行为逻辑保持原实现，迁移重点是模块边界调整。
- 结构测试已覆盖 `crop.rs` 的存在、接口 marker，以及 `viewer_page.rs` 的 helper absence checks。
- `viewer_page` 现有 crop 几何单测仍然在原测试入口下运行并通过。

## 疑虑

- `cargo test ui::viewer_page` 期间仍会打印 GTK theme parser warnings（`backdrop-filter`）。这属于仓库现有已知噪声，本次修改没有新增新的功能性告警或失败。
