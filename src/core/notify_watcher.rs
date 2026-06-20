//! 文件系统通知监听（增量更新）
//!
//! 启动一个阻塞线程，监听指定路径下的文件变化（创建 / 修改 / 删除 / 重命名）。
//! 当事件命中受支持的图片扩展名时，调用 [`LocalBackend::upsert_from_path`] 把最新
//! 的元数据写回 SQLite，从而让应用在不重新全量扫描的情况下保持索引与磁盘同步。
//!
//! 该模块与 [`crate::core::backend::scan_worker`] 互补：
//!   - `scan_worker` 在启动时做全量扫描；
//!   - `notify_watcher` 在运行期做增量更新。
use crate::core::backend::local::LocalBackend;
use crate::core::db::DbPool;
use notify::{event::EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;
use tokio::task::JoinHandle;

/// 启动后台文件监听，返回一个 `JoinHandle`。
///
/// 调用方应保留该句柄以便在关闭时 [`JoinHandle::abort`] 监听循环。
/// 监听在独立的阻塞线程中运行（`spawn_blocking`），不会阻塞 tokio / GTK 主循环。
pub fn start_watching(pool: DbPool, paths: Vec<PathBuf>) -> JoinHandle<()> {
    tokio::task::spawn_blocking(move || run_watcher_loop(pool, paths))
}

fn run_watcher_loop(pool: DbPool, paths: Vec<PathBuf>) {
    let (tx, rx) = mpsc::channel();
    let mut watcher: RecommendedWatcher = match notify::recommended_watcher(tx) {
        Ok(w) => w,
        Err(e) => {
            tracing::error!("watcher 创建失败: {}", e);
            return;
        }
    };

    for path in &paths {
        if let Err(e) = watcher.watch(path, RecursiveMode::Recursive) {
            tracing::warn!("监听 {} 失败: {}", path.display(), e);
        } else {
            tracing::info!("notify watcher 已启动: {}", path.display());
        }
    }

    // 持有 watcher —— 离开作用域时它会被 drop，所有监听自动停止。
    let backend = LocalBackend::new(pool);
    for evt in rx {
        handle_event(&backend, evt);
    }
    drop(watcher);
}

fn handle_event(backend: &LocalBackend, evt: Result<notify::Event, notify::Error>) {
    let evt = match evt {
        Ok(e) => e,
        Err(e) => {
            tracing::warn!("watcher 事件错误: {}", e);
            return;
        }
    };

    // 关注能反映磁盘内容变化的事件：创建 / 修改 / 重命名（touch）。
    // 删除事件不调用 upsert（仅记录日志），后续扫描会处理遗漏。
    let is_change = matches!(
        evt.kind,
        EventKind::Create(_) | EventKind::Modify(notify::event::ModifyKind::Data(_))
    );
    if !is_change {
        return;
    }

    for path in &evt.paths {
        if !is_supported_image(path) {
            continue;
        }
        if !path.is_file() {
            // 可能是目录事件或中间状态 —— 跳过即可。
            continue;
        }
        // 小延迟：让写入方完成文件落盘后再读取。
        std::thread::sleep(Duration::from_millis(50));
        if let Err(e) = backend.upsert_from_path(path) {
            tracing::warn!("upsert 失败 {}: {}", path.display(), e);
        } else {
            tracing::debug!("增量 upsert 成功: {}", path.display());
        }
    }
}

fn is_supported_image(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .as_deref(),
        Some("jpg") | Some("jpeg") | Some("png") | Some("webp") | Some("heic") | Some("heif")
    )
}
