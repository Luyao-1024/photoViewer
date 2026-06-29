//! 日志分类目标常量
//!
//! 这些常量用于 tracing 的 target 字段，实现日志分类过滤。
//!
//! 使用方式：
//! ```bash
//! # 仅显示 viewer 相关的 debug 日志
//! RUST_LOG=viewer=debug cargo run
//!
//! # 组合多个分类
//! RUST_LOG=viewer=debug,storage=warn cargo run
//!
//! # 显示所有 debug 日志
//! RUST_LOG=debug cargo run
//! ```
//!
//! 在代码中使用：
//! ```no_run
//! use photo_viewer::core::log_targets;
//!
//! tracing::debug!(target: log_targets::VIEWER, "message");
//! ```

/// 全屏查看器 — 导航、解码、缩略图栏、详情面板
pub const VIEWER: &str = "viewer";

/// 照片网格浏览 — 分组、布局、分页
pub const BROWSING: &str = "browsing";

/// 缩略图生成与加载
pub const THUMBNAILS: &str = "thumbnails";

/// 存储层 — 数据库、扫描、文件监听
pub const STORAGE: &str = "storage";

/// EXIF/视频元数据解析
pub const METADATA: &str = "metadata";

/// 编辑器操作 — 旋转、裁剪、保存
pub const EDITOR: &str = "editor";

/// 相册与回收站操作
pub const ALBUMS: &str = "albums";

/// 应用初始化与启动生命周期
pub const APP: &str = "app";
