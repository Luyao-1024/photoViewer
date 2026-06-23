//! 文件系统通知监听（增量更新）
//!
//! 启动一个阻塞线程，监听指定路径下的文件变化（创建 / 修改 / 删除 / 重命名）。
//! 当事件命中受支持的图片扩展名时，调用 [`LocalBackend::upsert_from_path`] 把最新
//! 的元数据写回 SQLite，并通过 [`MediaChangeNotifier`] 把"哪个 MediaItem 变了"
//! 推给 GTK 主线程的消费者（消费者负责把变更同步到 `media_list`）。
//!
//! 该模块与 [`crate::core::backend::scan_worker`] 互补：
//!   - `scan_worker` 在启动时做全量扫描；
//!   - `notify_watcher` 在运行期做增量更新。
use crate::core::albums;
use crate::core::backend::local::LocalBackend;
use crate::core::db::DbPool;
use crate::core::media_change_notifier::MediaChangeNotifier;
use notify::{event::EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;
use tokio::task::JoinHandle;

/// 启动后台文件监听，返回一个 `JoinHandle`。
///
/// 每次成功 upsert 之后，会通过 `notifier` 发出 `MediaChangeEvent::Upserted`
/// 事件；删除成功后发出 `MediaChangeEvent::Removed { uri }`。GTK 主线程
/// 的消费者负责把事件应用到 `media_list`。
///
/// 监听在独立的阻塞线程中运行（`spawn_blocking`），不会阻塞 tokio / GTK 主循环。
pub fn start_watching(
    pool: DbPool,
    paths: Vec<PathBuf>,
    notifier: MediaChangeNotifier,
) -> JoinHandle<()> {
    tokio::task::spawn_blocking(move || run_watcher_loop(pool, paths, notifier))
}

fn run_watcher_loop(pool: DbPool, paths: Vec<PathBuf>, notifier: MediaChangeNotifier) {
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
        handle_event(&backend, evt, &notifier);
    }
    drop(watcher);
}

fn handle_event(
    backend: &LocalBackend,
    evt: Result<notify::Event, notify::Error>,
    notifier: &MediaChangeNotifier,
) {
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
                match backend.upsert_from_path(path) {
                    Ok(Some(item)) => {
                        tracing::debug!("增量 upsert 成功: {}", path.display());
                        // albums 物化视图同步刷新（与 on_change 时机一致）。
                        if let Err(e) = albums::refresh(backend.pool()) {
                            tracing::warn!("albums::refresh after upsert failed: {}", e);
                        }
                        notifier.upserted(item);
                    }
                    Ok(None) => {
                        // 非文件 / 已消失；不通知 UI。
                    }
                    Err(e) => tracing::warn!("upsert 失败 {}: {}", path.display(), e),
                }
            }
        }
        EventKind::Remove(_) => {
            for path in &evt.paths {
                if !is_supported_image(path) {
                    continue;
                }
                let uri = format!("file://{}", path.display());
                match backend.delete_path(path) {
                    Ok(changed) if changed > 0 => {
                        tracing::debug!("增量删除成功: {}", path.display());
                        if let Err(e) = albums::refresh(backend.pool()) {
                            tracing::warn!("albums::refresh after delete failed: {}", e);
                        }
                        notifier.removed(uri);
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
                    match backend.upsert_from_path(path) {
                        Ok(Some(item)) => {
                            tracing::debug!("rename upsert 成功: {}", path.display());
                            if let Err(e) = albums::refresh(backend.pool()) {
                                tracing::warn!("albums::refresh after rename upsert failed: {}", e);
                            }
                            notifier.upserted(item);
                        }
                        Ok(None) => {}
                        Err(e) => tracing::warn!("rename upsert 失败 {}: {}", path.display(), e),
                    }
                } else {
                    let uri = format!("file://{}", path.display());
                    match backend.delete_path(path) {
                        Ok(changed) if changed > 0 => {
                            if let Err(e) = albums::refresh(backend.pool()) {
                                tracing::warn!("albums::refresh after rename delete failed: {}", e);
                            }
                            notifier.removed(uri);
                        }
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
    use crate::core::media_change_notifier::MediaChangeEvent;
    use chrono::Utc;
    use notify::{event::RemoveKind, Event};

    #[test]
    fn remove_event_deletes_media_row_and_emits_removed_event() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gone.jpg");
        std::fs::write(&path, b"not actually decoded in this test").unwrap();
        let pool = db::init_pool(&dir.path().join("test.db")).unwrap();
        let uri = format!("file://{}", path.display());
        db::insert_media_item(
            &pool,
            &NewMediaItem {
                uri: uri.clone(),
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
        let (notifier, mut rx) = crate::core::media_change_notifier::MediaChangeNotifier::new();
        handle_event(
            &backend,
            Ok(Event {
                kind: EventKind::Remove(RemoveKind::File),
                paths: vec![path],
                attrs: Default::default(),
            }),
            &notifier,
        );

        assert!(db::list_all_media(&pool).unwrap().is_empty());
        match rx.try_recv() {
            Ok(MediaChangeEvent::Removed { uri: received }) => assert_eq!(received, uri),
            other => panic!("expected Removed, got {other:?}"),
        }
    }
}
