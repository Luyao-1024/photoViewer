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
/// `on_change` 在每次成功 upsert 之后被同步调用一次，调用方负责把
/// "索引有变化" 信号转发到 GTK 主线程并刷新聚合视图（如 `albums::refresh`）。
///
/// 调用方应保留该句柄以便在关闭时 [`JoinHandle::abort`] 监听循环。
/// 监听在独立的阻塞线程中运行（`spawn_blocking`），不会阻塞 tokio / GTK 主循环。
pub fn start_watching<F>(pool: DbPool, paths: Vec<PathBuf>, on_change: F) -> JoinHandle<()>
where
    F: Fn() + Send + 'static,
{
    tokio::task::spawn_blocking(move || run_watcher_loop(pool, paths, on_change))
}

fn run_watcher_loop<F>(pool: DbPool, paths: Vec<PathBuf>, on_change: F)
where
    F: Fn() + Send + 'static,
{
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
        handle_event(&backend, evt, &on_change);
    }
    drop(watcher);
}

fn handle_event<F>(backend: &LocalBackend, evt: Result<notify::Event, notify::Error>, on_change: &F)
where
    F: Fn() + Send + 'static,
{
    let evt = match evt {
        Ok(e) => e,
        Err(e) => {
            tracing::warn!("watcher 事件错误: {}", e);
            return;
        }
    };

    match evt.kind {
        EventKind::Create(_) | EventKind::Modify(notify::event::ModifyKind::Data(_)) => {
            for path in &evt.paths {
                if !is_supported_image(path) {
                    continue;
                }
                if !path.is_file() {
                    continue;
                }
                std::thread::sleep(Duration::from_millis(50));
                if let Err(e) = backend.upsert_from_path(path) {
                    tracing::warn!("upsert 失败 {}: {}", path.display(), e);
                } else {
                    tracing::debug!("增量 upsert 成功: {}", path.display());
                    on_change();
                }
            }
        }
        EventKind::Remove(_) => {
            for path in &evt.paths {
                if !is_supported_image(path) {
                    continue;
                }
                match backend.delete_path(path) {
                    Ok(changed) if changed > 0 => {
                        tracing::debug!("增量删除成功: {}", path.display());
                        on_change();
                    }
                    Ok(_) => {}
                    Err(e) => tracing::warn!("增量删除失败 {}: {}", path.display(), e),
                }
            }
        }
        EventKind::Modify(notify::event::ModifyKind::Name(_)) => {
            for path in &evt.paths {
                if !is_supported_image(path) {
                    continue;
                }
                if path.is_file() {
                    std::thread::sleep(Duration::from_millis(50));
                    if let Err(e) = backend.upsert_from_path(path) {
                        tracing::warn!("rename upsert 失败 {}: {}", path.display(), e);
                    } else {
                        on_change();
                    }
                } else {
                    match backend.delete_path(path) {
                        Ok(changed) if changed > 0 => on_change(),
                        Ok(_) => {}
                        Err(e) => tracing::warn!("rename delete 失败 {}: {}", path.display(), e),
                    }
                }
            }
        }
        _ => {}
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::db;
    use crate::core::media::NewMediaItem;
    use chrono::Utc;
    use notify::{event::RemoveKind, Event};
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    #[test]
    fn remove_event_deletes_media_row_and_notifies_change() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gone.jpg");
        std::fs::write(&path, b"not actually decoded in this test").unwrap();
        let pool = db::init_pool(&dir.path().join("test.db")).unwrap();
        db::insert_media_item(
            &pool,
            &NewMediaItem {
                uri: format!("file://{}", path.display()),
                path: path.clone(),
                folder_path: dir.path().to_path_buf(),
                mime_type: "image/jpeg".into(),
                width: None,
                height: None,
                taken_at: None,
                file_mtime: Utc::now(),
                file_size: 1,
                blake3_hash: "hash".into(),
            },
        )
        .unwrap();
        std::fs::remove_file(&path).unwrap();

        let backend = LocalBackend::new(pool.clone());
        let changes = Arc::new(AtomicUsize::new(0));
        let changes_for_cb = changes.clone();
        handle_event(
            &backend,
            Ok(Event {
                kind: EventKind::Remove(RemoveKind::File),
                paths: vec![path],
                attrs: Default::default(),
            }),
            &move || {
                changes_for_cb.fetch_add(1, Ordering::SeqCst);
            },
        );

        assert!(db::list_all_media(&pool).unwrap().is_empty());
        assert_eq!(changes.load(Ordering::SeqCst), 1);
    }
}
