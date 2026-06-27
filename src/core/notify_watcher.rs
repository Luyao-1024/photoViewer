//! 文件系统通知监听（增量更新）
//!
//! 启动一个阻塞线程，监听指定路径下的文件变化（创建 / 修改 / 删除 / 重命名）。
//! 当事件命中受支持的图片扩展名时，调用 [`LocalBackend::upsert_from_path`] 把最新
//! 的元数据写回 SQLite，并通过 [`MediaChangeNotifier`] 把"哪个 MediaItem 变了"
//! 推给 GTK 主线程的消费者（消费者负责把变更同步到 `media_list`）。
//!
//! 除了相册目录，还会监听**系统回收站根**：外部（文件管理器）对回收站的操作
//!（还原 / 清空 / 从回收站删除）只动回收站目录、不动相册目录，必须单独监听才能
//! 实时感知。回收站事件经防抖合并后跑一次 [`trash::reconcile_trash`]，并广播
//! [`MediaChangeEvent::TrashChanged`]，让可见的回收站页面实时刷新。
//!
//! 该模块与 [`crate::core::backend::scan_worker`] 互补：
//!   - `scan_worker` 在启动时做全量扫描；
//!   - `notify_watcher` 在运行期做增量更新。
use crate::core::albums;
use crate::core::backend::local::LocalBackend;
use crate::core::db::DbPool;
use crate::core::media::is_supported_media_path;
use crate::core::media_change_notifier::MediaChangeNotifier;
use crate::core::trash;
use notify::{event::EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;
use tokio::task::JoinHandle;

/// 回收站事件防抖静默期：gio/gvfs 一次操作（尤其"清空"）会产生大量事件，合并成
/// 一次对账 + 一次刷新。
const TRASH_DEBOUNCE: Duration = Duration::from_millis(400);

/// 启动后台文件监听，返回一个 `JoinHandle`。
///
/// `watch_paths` 是要安装 inotify 的目录（相册目录 + 存在的回收站根）；
/// `trash_roots` 用于把事件分类成"回收站事件"（路径落在某个回收站根下）；
/// `pictures_root` 是对账时判断"原路径是否属于本图库"的根。
///
/// 监听在独立的阻塞线程中运行（`spawn_blocking`），不会阻塞 tokio / GTK 主循环。
pub fn start_watching(
    pool: DbPool,
    watch_paths: Vec<PathBuf>,
    trash_roots: Vec<PathBuf>,
    pictures_root: PathBuf,
    notifier: MediaChangeNotifier,
) -> JoinHandle<()> {
    tokio::task::spawn_blocking(move || {
        run_watcher_loop(pool, watch_paths, trash_roots, pictures_root, notifier)
    })
}

fn run_watcher_loop(
    pool: DbPool,
    watch_paths: Vec<PathBuf>,
    trash_roots: Vec<PathBuf>,
    pictures_root: PathBuf,
    notifier: MediaChangeNotifier,
) {
    let (tx, rx) = mpsc::channel();
    let mut watcher: RecommendedWatcher = match notify::recommended_watcher(tx) {
        Ok(w) => w,
        Err(e) => {
            tracing::error!("watcher 创建失败: {}", e);
            return;
        }
    };

    for path in &watch_paths {
        if let Err(e) = watcher.watch(path, RecursiveMode::Recursive) {
            tracing::warn!("监听 {} 失败: {}", path.display(), e);
        } else {
            tracing::info!("notify watcher 已启动: {}", path.display());
        }
    }

    // 持有 watcher —— 离开作用域时它会被 drop，所有监听自动停止。
    let backend = LocalBackend::new(pool.clone());
    let mut trash_dirty = false;

    while let Ok(evt) = rx.recv() {
        dispatch_event(&backend, evt, &notifier, &trash_roots, &mut trash_dirty);

        // 排空本轮事件突发；静默 TRASH_DEBOUNCE（或通道关闭）后，若有回收站事件则
        // 对账 + 通知。
        while let Ok(e) = rx.recv_timeout(TRASH_DEBOUNCE) {
            dispatch_event(&backend, e, &notifier, &trash_roots, &mut trash_dirty);
        }
        flush_trash_reconcile(&pool, &pictures_root, &notifier, &mut trash_dirty);
    }
    // 通道关闭（停监）：把挂起的回收站变化最后冲刷一次再退出。
    flush_trash_reconcile(&pool, &pictures_root, &notifier, &mut trash_dirty);
    drop(watcher);
}

/// 把一条事件分发到"回收站对账"或"相册增量 upsert/delete"。
///
/// 路径落在任一回收站根下 → 回收站事件（只置脏位，等防抖后批量对账）；否则按相册
/// 事件走 [`handle_event`]。
fn dispatch_event(
    backend: &LocalBackend,
    evt: Result<notify::Event, notify::Error>,
    notifier: &MediaChangeNotifier,
    trash_roots: &[PathBuf],
    trash_dirty: &mut bool,
) {
    let evt = match evt {
        Ok(e) => e,
        Err(e) => {
            tracing::warn!("watcher 事件错误: {}", e);
            return;
        }
    };
    if evt.paths.iter().any(|p| is_under_trash(p, trash_roots)) {
        *trash_dirty = true;
        return;
    }
    handle_event(backend, Ok(evt), notifier);
}

fn is_under_trash(path: &Path, trash_roots: &[PathBuf]) -> bool {
    trash_roots.iter().any(|root| path.starts_with(root))
}

/// 若有挂起的回收站事件：跑一次对账（add + prune 收敛 DB），并广播 TrashChanged
/// 让可见的回收站页面刷新。始终广播——还原等操作的 DB 变更可能由相册监听器完成，
/// 对账本身未必改库，但回收站视图仍需重读。
fn flush_trash_reconcile(
    pool: &DbPool,
    pictures_root: &Path,
    notifier: &MediaChangeNotifier,
    trash_dirty: &mut bool,
) {
    if !*trash_dirty {
        return;
    }
    *trash_dirty = false;
    if let Err(e) = trash::reconcile_trash(pool, pictures_root) {
        tracing::warn!("watcher 触发的回收站对账失败: {e}");
    }
    notifier.trash_changed();
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
                if !is_supported_media_path(path) {
                    continue;
                }
                if !path.is_file() {
                    continue;
                }
                std::thread::sleep(Duration::from_millis(50));
                match backend.upsert_from_path(path) {
                    Ok(Some(item)) => {
                        tracing::debug!(target: crate::core::log_targets::STORAGE, "增量 upsert 成功: {}", path.display());
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
                if !is_supported_media_path(path) {
                    continue;
                }
                let uri = format!("file://{}", path.display());
                match backend.delete_path(path) {
                    Ok(changed) if changed > 0 => {
                        tracing::debug!(target: crate::core::log_targets::STORAGE, "增量删除成功: {}", path.display());
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
                if !is_supported_media_path(path) {
                    continue;
                }
                if path.is_file() {
                    std::thread::sleep(Duration::from_millis(50));
                    match backend.upsert_from_path(path) {
                        Ok(Some(item)) => {
                            tracing::debug!(target: crate::core::log_targets::STORAGE, "rename upsert 成功: {}", path.display());
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

    #[test]
    fn remove_event_accepts_video_media_extension() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gone.mp4");
        std::fs::write(&path, b"not actually decoded in this test").unwrap();
        let pool = db::init_pool(&dir.path().join("test.db")).unwrap();
        let uri = format!("file://{}", path.display());
        db::insert_media_item(
            &pool,
            &NewMediaItem {
                uri: uri.clone(),
                path: path.clone(),
                folder_path: dir.path().to_path_buf(),
                mime_type: "video/mp4".into(),
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
            other => panic!("expected Removed for video, got {other:?}"),
        }
    }

    /// Regression: 删除到回收站后 gio 把文件移出受监听目录，watcher 会收到
    /// 原路径的 Remove 事件。但该行的 `trashed_at` 已被应用置位（`move_to_trash`
    /// 之后的 `db::mark_trashed`）——watcher 绝不能把它从 DB 硬删，否则
    /// `list_trashed_media` 返回空、回收站页面"看不见图片"。之前这正是
    /// `delete_media_by_path` 无条件按 path/uri 删行导致的。
    #[test]
    fn remove_event_keeps_trashed_row_for_trash_page() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("trashed.jpg");
        std::fs::write(&path, b"not actually decoded in this test").unwrap();
        let pool = db::init_pool(&dir.path().join("test.db")).unwrap();
        let id = db::insert_media_item(
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
        // 应用侧已标记回收站（move_to_trash 之后的 db::mark_trashed）
        db::mark_trashed(&pool, id).unwrap();
        // gio 已把文件移走 → watcher 看到原路径的 Remove 事件
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

        // 回收站行必须保留，回收站页面才看得见
        assert_eq!(
            db::list_trashed_media(&pool).unwrap().len(),
            1,
            "watcher must not hard-delete a row the app marked trashed"
        );
        // 且不应广播 Removed 事件让 UI 把它也从实时列表抹掉
        assert!(
            rx.try_recv().is_err(),
            "watcher must not emit Removed for a trashed row"
        );
    }

    #[test]
    fn is_under_trash_classifies_paths_under_trash_roots() {
        let roots = vec![PathBuf::from("/home/u/.local/share/Trash")];
        assert!(is_under_trash(
            Path::new("/home/u/.local/share/Trash/files/x.jpg"),
            &roots
        ));
        assert!(is_under_trash(
            Path::new("/home/u/.local/share/Trash/info/x.jpg.trashinfo"),
            &roots
        ));
        assert!(!is_under_trash(
            Path::new("/home/u/图片/Camera/x.jpg"),
            &roots
        ));
        assert!(!is_under_trash(
            Path::new("/home/u/.local/share/other"),
            &roots
        ));
    }

    /// 落在回收站根下的事件只置脏位（等防抖对账），不应走 handle_event——否则会把
    /// 回收站里的文件当相册文件 upsert/delete。
    #[test]
    fn dispatch_event_marks_trash_dirty_without_running_handle_event() {
        let dir = tempfile::tempdir().unwrap();
        let pool = db::init_pool(&dir.path().join("t.db")).unwrap();
        let backend = LocalBackend::new(pool);
        let (notifier, mut rx) = MediaChangeNotifier::new();
        let trash_roots = vec![dir.path().join("Trash")];
        let mut dirty = false;

        dispatch_event(
            &backend,
            Ok(Event {
                kind: EventKind::Remove(RemoveKind::File),
                paths: vec![dir.path().join("Trash").join("files").join("x.jpg")],
                attrs: Default::default(),
            }),
            &notifier,
            &trash_roots,
            &mut dirty,
        );

        assert!(dirty, "a trash-dir event must set the dirty flag");
        assert!(
            rx.try_recv().is_err(),
            "a trash-dir event must not trigger handle_event / notifier"
        );
    }
}
