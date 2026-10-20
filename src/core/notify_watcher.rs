//! 文件系统通知监听（增量更新）
//!
//! 启动一个阻塞线程，监听指定路径下的文件变化（创建 / 修改 / 删除 / 重命名）。
//! 当事件命中受支持的图片扩展名时，先在 watcher 后台线程提取元数据，再把 DB 提交
//! 发送给 [`crate::core::db_actor::DbActorHandle`]。actor 串行写 SQLite 并发出
//! domain event 给 GTK 主线程消费者（消费者负责把变更同步到 `media_list`）。
//!
//! 除了相册目录，还会监听**系统回收站根**：外部（文件管理器）对回收站的操作
//!（还原 / 清空 / 从回收站删除）只动回收站目录、不动相册目录，必须单独监听才能
//! 实时感知。回收站事件经防抖合并后跑一次 [`trash::reconcile_trash`]，并广播
//! [`crate::core::events::DomainEvent::TrashChanged`]，让可见的回收站页面实时刷新。
//!
//! 该模块与 [`crate::core::backend::scan_worker`] 互补：
//!   - `scan_worker` 在启动时做全量扫描；
//!   - `notify_watcher` 在运行期做增量更新。
use crate::core::backend::local::LocalBackend;
use crate::core::db_actor::{DbActorHandle, DbCommand};
use crate::core::events::ChangeSource;
use crate::core::log_targets;
use crate::core::media::is_supported_media_path;
use crate::core::runtime_config;
use crate::core::telemetry::{log_error, log_warning, OperationTrace, TraceChain};
use notify::{event::EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::task::JoinHandle;

/// Bound on the buffered filesystem events. A burst larger than this is treated
/// as overflow and answered with a full rescan rather than unbounded memory.
const EVENT_QUEUE_CAPACITY: usize = 4096;

/// 启动后台文件监听，返回一个 `JoinHandle`。
///
/// `watch_paths` 是要安装 inotify 的目录（相册目录 + 存在的回收站根）；
/// `trash_roots` 用于把事件分类成"回收站事件"（路径落在某个回收站根下）；
/// `media_roots` 是对账时判断"原路径是否属于本图库"的根集合。
///
/// 监听在独立的阻塞线程中运行（`spawn_blocking`），不会阻塞 tokio / GTK 主循环。
pub fn start_watching(
    db_actor: DbActorHandle,
    watch_paths: Vec<PathBuf>,
    trash_roots: Vec<PathBuf>,
    excluded_roots: Vec<PathBuf>,
    media_roots: Vec<PathBuf>,
) -> WatcherHandle {
    // The watcher — and with it the event callback that owns the only sender of
    // the event channel — lives in the handle, not in the loop.
    //
    // The loop ends when that channel closes, and the channel closes when the
    // watcher is dropped, so the two must not be owned by the same scope: held
    // together they wait for each other and the blocking task never returns.
    // Owning the watcher here is what makes [`WatcherHandle::stop`] able to
    // break that cycle.
    let watcher_slot: Arc<Mutex<Option<RecommendedWatcher>>> = Arc::new(Mutex::new(None));
    let overflowed = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::sync_channel(EVENT_QUEUE_CAPACITY);

    let mut watcher = match build_watcher(&tx, &overflowed) {
        Ok(watcher) => watcher,
        Err(error) => {
            tracing::error!(target: log_targets::STORAGE, "watcher 创建失败: {}", error);
            return WatcherHandle {
                watcher: watcher_slot,
                join: None,
            };
        }
    };
    for path in &watch_paths {
        if let Err(error) = watcher.watch(path, RecursiveMode::Recursive) {
            tracing::warn!(target: log_targets::STORAGE, "监听 {} 失败: {}", path.display(), error);
        } else {
            tracing::info!(target: log_targets::STORAGE, "notify watcher 已启动: {}", path.display());
        }
    }

    let watcher_for_loop = watcher_slot.clone();
    let join = tokio::task::spawn_blocking(move || {
        run_watcher_loop(
            db_actor,
            rx,
            overflowed,
            watcher_for_loop,
            trash_roots,
            excluded_roots,
            media_roots,
        )
    });
    *watcher_slot.lock().expect("watcher slot is not poisoned") = Some(watcher);

    WatcherHandle {
        watcher: watcher_slot,
        join: Some(join),
    }
}

/// Keeps the watcher alive and lets a caller stop listening.
///
/// `JoinHandle::abort` cannot cancel a `spawn_blocking` task, so aborting one of
/// these handles does nothing at all: the blocking loop keeps running and any
/// `Runtime` that owns it hangs forever in its shutdown, waiting for a task that
/// was never cancelled. Call [`WatcherHandle::stop`] instead.
pub struct WatcherHandle {
    watcher: Arc<Mutex<Option<RecommendedWatcher>>>,
    join: Option<JoinHandle<()>>,
}

impl WatcherHandle {
    /// Stop listening.
    ///
    /// Dropping the watcher closes the event channel, which ends the loop's
    /// `recv`, so the blocking task returns and an owning `Runtime` can shut
    /// down normally instead of waiting forever for a task that was never
    /// cancelled. Idempotent.
    pub fn stop(&self) {
        let taken = self
            .watcher
            .lock()
            .expect("watcher slot is not poisoned")
            .take();
        if taken.is_some() {
            tracing::info!(
                target: log_targets::STORAGE,
                "notify watcher 已停止：事件通道关闭，监听循环将退出",
            );
        }
        drop(taken);
        if let Some(join) = self.join.as_ref() {
            tracing::trace!(
                target: log_targets::STORAGE,
                "notify watcher 任务已结束 = {}",
                join.is_finished(),
            );
        }
    }
}

impl Drop for WatcherHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

fn build_watcher(
    tx: &mpsc::SyncSender<Result<notify::Event, notify::Error>>,
    overflowed: &Arc<AtomicBool>,
) -> notify::Result<RecommendedWatcher> {
    let tx = tx.clone();
    let overflowed_for_handler = overflowed.clone();
    notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        if event.is_err() {
            overflowed_for_handler.store(true, Ordering::Release);
        }
        if let Err(mpsc::TrySendError::Full(_)) = tx.try_send(event) {
            overflowed_for_handler.store(true, Ordering::Release);
        }
    })
}

fn run_watcher_loop(
    db_actor: DbActorHandle,
    rx: mpsc::Receiver<Result<notify::Event, notify::Error>>,
    overflowed: Arc<AtomicBool>,
    _watcher_slot: Arc<Mutex<Option<RecommendedWatcher>>>,
    trash_roots: Vec<PathBuf>,
    excluded_roots: Vec<PathBuf>,
    media_roots: Vec<PathBuf>,
) {
    const MAX_BURST_EVENTS: usize = 512;
    const MAX_BURST_AGE: Duration = Duration::from_millis(750);

    // The watcher itself is owned by the `WatcherHandle`, not by this loop: the
    // loop ends when the event channel closes, and the channel closes when the
    // watcher is dropped, so holding both in one scope would make each wait for
    // the other forever. `_watcher_slot` is a handle to the same slot purely so
    // the loop cannot outlive the stop signal being consumed elsewhere.
    let mut trash_dirty = false;

    while let Ok(evt) = rx.recv() {
        let mut burst = vec![evt];
        let burst_started = Instant::now();
        // Collect the whole notification burst before doing metadata work.
        // Editors commonly emit several writes/renames for one path; only the
        // final observable state should reach extraction and the DB actor.
        while burst.len() < MAX_BURST_EVENTS && burst_started.elapsed() < MAX_BURST_AGE {
            let Ok(e) = rx.recv_timeout(Duration::from_millis(
                runtime_config::notify_trash_debounce_ms(),
            )) else {
                break;
            };
            burst.push(e);
        }
        flush_event_burst(
            &db_actor,
            burst,
            &trash_roots,
            &excluded_roots,
            &mut trash_dirty,
        );
        flush_trash_reconcile(&db_actor, &media_roots, &mut trash_dirty);
        if overflowed.swap(false, Ordering::AcqRel) {
            recover_from_event_overflow(&db_actor, &media_roots, &excluded_roots);
        }
    }
    // 通道关闭（停监）：把挂起的回收站变化最后冲刷一次再退出。
    flush_trash_reconcile(&db_actor, &media_roots, &mut trash_dirty);
}

fn recover_from_event_overflow(
    db_actor: &DbActorHandle,
    media_roots: &[PathBuf],
    excluded_roots: &[PathBuf],
) {
    let trace = OperationTrace::start(TraceChain::Filesystem, "watcher_overflow_rescan");
    let backend = LocalBackend::new(db_actor.pool().clone());
    for root in media_roots {
        if !crate::core::backend::local::scan_root_is_available(root) {
            continue;
        }
        let actor = db_actor.clone();
        let trace_for_commit = trace.clone();
        if let Err(error) = backend.scan_and_submit_dir_notify_with_exclusions(
            root,
            excluded_roots,
            move |items| match actor.execute_blocking_in_trace(
                trace_for_commit.clone(),
                DbCommand::UpsertMediaBatch {
                    source: ChangeSource::FilesystemWatcher,
                    items,
                },
            )? {
                crate::core::db_actor::DbCommandResult::MediaItems(items) => Ok(items),
                other => Err(crate::core::error::AppError::Backend(format!(
                    "unexpected overflow rescan response: {other:?}"
                ))),
            },
            |_| {},
        ) {
            log_error(&trace, "overflow_rescan", error);
        }
    }
    match backend.collect_missing_live_media_under_roots(media_roots, excluded_roots) {
        Ok(missing) if !missing.is_empty() => {
            if let Err(error) = db_actor.execute_blocking_in_trace(
                trace.clone(),
                DbCommand::PruneMissingLiveRows { missing },
            ) {
                log_error(&trace, "overflow_prune", error);
            }
        }
        Ok(_) => {}
        Err(error) => log_error(&trace, "overflow_collect_missing", error),
    }
}

#[derive(Clone, Copy)]
enum PendingPathAction {
    Upsert,
    Delete,
    /// A directory vanished; every row at or below it must go.
    DeleteTree,
    /// A directory arrived (a rename within the library); index its contents.
    ScanDirectory,
}

/// Decide what one path of an event means.
///
/// Delete classification must not require a media extension: a file manager
/// deleting a folder reports only the folder path, and dropping it here is why
/// the app used to keep showing the folder's photos until the next scan.
fn classify_path_action(kind: &EventKind, path: &Path) -> Option<PendingPathAction> {
    match kind {
        EventKind::Create(_) | EventKind::Modify(notify::event::ModifyKind::Data(_)) => {
            is_supported_media_path(path).then_some(PendingPathAction::Upsert)
        }
        EventKind::Remove(kind) => match kind {
            notify::event::RemoveKind::Folder => Some(PendingPathAction::DeleteTree),
            notify::event::RemoveKind::File => {
                is_supported_media_path(path).then_some(PendingPathAction::Delete)
            }
            // Backends that cannot tell files from folders: a supported
            // extension is certainly a file; anything else may be a folder.
            _ => Some(if is_supported_media_path(path) {
                PendingPathAction::Delete
            } else {
                PendingPathAction::DeleteTree
            }),
        },
        EventKind::Modify(notify::event::ModifyKind::Name(_)) => {
            if path.is_file() {
                is_supported_media_path(path).then_some(PendingPathAction::Upsert)
            } else if path.is_dir() {
                Some(PendingPathAction::ScanDirectory)
            } else if is_supported_media_path(path) {
                Some(PendingPathAction::Delete)
            } else {
                Some(PendingPathAction::DeleteTree)
            }
        }
        _ => None,
    }
}

fn flush_event_burst(
    db_actor: &DbActorHandle,
    events: Vec<Result<notify::Event, notify::Error>>,
    trash_roots: &[PathBuf],
    excluded_roots: &[PathBuf],
    trash_dirty: &mut bool,
) {
    let current_excluded_roots = crate::core::prefs::excluded_scan_roots();
    let mut pending = HashMap::<PathBuf, PendingPathAction>::new();
    for event in events {
        let event = match event {
            Ok(event) => event,
            Err(error) => {
                tracing::warn!("watcher 事件错误: {error}");
                continue;
            }
        };
        for path in event.paths {
            if is_under_trash(&path, trash_roots) {
                *trash_dirty = true;
                continue;
            }
            if is_under_effective_excluded(&path, excluded_roots, current_excluded_roots.as_slice())
            {
                continue;
            }
            if let Some(action) = classify_path_action(&event.kind, &path) {
                pending.insert(path, action);
            }
        }
    }
    let mut scan_exclusions = excluded_roots.to_vec();
    for root in current_excluded_roots {
        if !scan_exclusions.contains(&root) {
            scan_exclusions.push(root);
        }
    }
    flush_pending_paths(db_actor, pending, &scan_exclusions);
}

fn flush_pending_paths(
    db_actor: &DbActorHandle,
    pending: HashMap<PathBuf, PendingPathAction>,
    excluded_roots: &[PathBuf],
) {
    if pending.is_empty() {
        return;
    }
    let trace = OperationTrace::start(TraceChain::Filesystem, "watcher_burst");
    let has_upserts = pending.values().any(|action| {
        matches!(
            action,
            PendingPathAction::Upsert | PendingPathAction::ScanDirectory
        )
    });
    if has_upserts {
        std::thread::sleep(Duration::from_millis(
            runtime_config::notify_file_settle_ms(),
        ));
    }

    let mut items = Vec::new();
    let mut deletes = Vec::new();
    let mut tree_deletes = Vec::new();
    let mut scans = Vec::new();
    for (path, action) in pending {
        match action {
            PendingPathAction::Upsert if path.is_file() => {
                match LocalBackend::new_item_from_path(&path) {
                    Ok(Some(item)) => items.push(item),
                    Ok(None) => {}
                    Err(error) => log_warning(
                        &trace,
                        "metadata_extract",
                        format!("{}: {error}", path.display()),
                    ),
                }
            }
            PendingPathAction::Upsert | PendingPathAction::Delete => deletes.push(path),
            PendingPathAction::DeleteTree => tree_deletes.push(path),
            PendingPathAction::ScanDirectory => scans.push(path),
        }
    }
    // A recursive move into the library reports only the top folder; scanning
    // a folder already covered by an ancestor's scan would index it twice.
    for root in topmost_directories(scans) {
        tracing::info!(
            target: log_targets::STORAGE,
            "watcher 目录出现，扫描其内容: {}",
            root.display()
        );
        scan_directory_tree(db_actor, &root, excluded_roots);
    }
    if !items.is_empty() {
        if let Err(error) = db_actor.execute_blocking_in_trace(
            trace.clone(),
            DbCommand::UpsertMediaBatch {
                source: ChangeSource::FilesystemWatcher,
                items,
            },
        ) {
            log_error(&trace, "db_upsert_batch", &error);
        }
    }
    if !deletes.is_empty() {
        tracing::info!(
            target: log_targets::STORAGE,
            "watcher 文件消失，删除对应行: {:?}",
            deletes
        );
        if let Err(error) = db_actor.execute_blocking_in_trace(
            trace.clone(),
            DbCommand::DeleteLiveByPaths {
                source: ChangeSource::FilesystemWatcher,
                paths: deletes,
            },
        ) {
            log_error(&trace, "db_delete_batch", &error);
        }
    }
    if !tree_deletes.is_empty() {
        tracing::info!(
            target: log_targets::STORAGE,
            "watcher 目录消失，删除其下所有行: {:?}",
            tree_deletes
        );
        if let Err(error) = db_actor.execute_blocking_in_trace(
            trace.clone(),
            DbCommand::DeleteLiveByFolderTrees {
                source: ChangeSource::FilesystemWatcher,
                paths: tree_deletes,
            },
        ) {
            log_error(&trace, "db_delete_folder_trees", &error);
        }
    }
}

/// Keep only the outermost of a set of directories, so a moved tree is walked
/// once instead of once per reported subdirectory.
fn topmost_directories(mut dirs: Vec<PathBuf>) -> Vec<PathBuf> {
    dirs.sort();
    dirs.dedup();
    let mut kept: Vec<PathBuf> = Vec::new();
    for dir in dirs {
        if kept.iter().any(|root| dir.starts_with(root)) {
            continue;
        }
        kept.push(dir);
    }
    kept
}

/// Index every supported file below a directory that arrived through a rename.
/// The individual children of a moved directory never produce their own
/// events, so without this an in-library folder rename would delete the old
/// rows and never add the new ones.
fn scan_directory_tree(db_actor: &DbActorHandle, root: &Path, excluded_roots: &[PathBuf]) {
    if !crate::core::backend::local::scan_root_is_available(root) {
        return;
    }
    let trace = OperationTrace::start(TraceChain::Filesystem, "watcher_scan_dir");
    let backend = LocalBackend::new(db_actor.pool().clone());
    let actor = db_actor.clone();
    let trace_for_commit = trace.clone();
    if let Err(error) = backend.scan_and_submit_dir_notify_with_exclusions(
        root,
        excluded_roots,
        move |items| match actor.execute_blocking_in_trace(
            trace_for_commit.clone(),
            DbCommand::UpsertMediaBatch {
                source: ChangeSource::FilesystemWatcher,
                items,
            },
        )? {
            crate::core::db_actor::DbCommandResult::MediaItems(items) => Ok(items),
            other => Err(crate::core::error::AppError::Backend(format!(
                "unexpected directory scan response: {other:?}"
            ))),
        },
        |_| {},
    ) {
        log_error(&trace, "scan_dir", &error);
    }
}

/// 把一条事件分发到"回收站对账"或"相册增量 upsert/delete"。
///
/// 路径落在任一回收站根下 → 回收站事件（只置脏位，等防抖后批量对账）；否则按相册
/// 事件走 [`handle_event`]。
#[cfg(test)]
fn dispatch_event(
    db_actor: &DbActorHandle,
    evt: Result<notify::Event, notify::Error>,
    trash_roots: &[PathBuf],
    excluded_roots: &[PathBuf],
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
    let current_excluded_roots = crate::core::prefs::excluded_scan_roots();
    if evt
        .paths
        .iter()
        .any(|p| is_under_effective_excluded(p, excluded_roots, current_excluded_roots.as_slice()))
    {
        return;
    }
    handle_event(db_actor, Ok(evt));
}

fn is_under_trash(path: &Path, trash_roots: &[PathBuf]) -> bool {
    trash_roots.iter().any(|root| path.starts_with(root))
}

fn is_under_excluded(path: &Path, excluded_roots: &[PathBuf]) -> bool {
    excluded_roots.iter().any(|root| path.starts_with(root))
}

fn is_under_effective_excluded(
    path: &Path,
    startup_excluded_roots: &[PathBuf],
    current_excluded_roots: &[PathBuf],
) -> bool {
    is_under_excluded(path, startup_excluded_roots)
        || is_under_excluded(path, current_excluded_roots)
}

/// 若有挂起的回收站事件：跑一次对账（add + prune 收敛 DB），并广播 TrashChanged
/// 让可见的回收站页面刷新。始终广播——还原等操作的 DB 变更可能由相册监听器完成，
/// 对账本身未必改库，但回收站视图仍需重读。
fn flush_trash_reconcile(
    db_actor: &DbActorHandle,
    media_roots: &[PathBuf],
    trash_dirty: &mut bool,
) {
    if !*trash_dirty {
        return;
    }
    *trash_dirty = false;
    let trace = OperationTrace::start(TraceChain::Filesystem, "reconcile_trash");
    let plan =
        match crate::core::trash::prepare_trash_reconcile_for_roots(db_actor.pool(), media_roots) {
            Ok(plan) => plan,
            Err(error) => {
                log_error(&trace, "prepare_reconcile", &error);
                return;
            }
        };
    let result =
        db_actor.execute_blocking_in_trace(trace.clone(), DbCommand::ReconcileTrash { plan });
    if let Err(error) = result {
        log_error(&trace, "db_reconcile", &error);
    }
}

#[cfg(test)]
fn handle_event(db_actor: &DbActorHandle, evt: Result<notify::Event, notify::Error>) {
    let evt = match evt {
        Ok(e) => e,
        Err(e) => {
            tracing::warn!("watcher 事件错误: {}", e);
            return;
        }
    };
    let trace = OperationTrace::start(TraceChain::Filesystem, "watcher_event");
    let stage = trace.stage("dispatch");
    stage.record("item_count", evt.paths.len());

    match evt.kind {
        EventKind::Create(_) | EventKind::Modify(notify::event::ModifyKind::Data(_)) => {
            for path in &evt.paths {
                if !is_supported_media_path(path) {
                    continue;
                }
                if !path.is_file() {
                    continue;
                }
                std::thread::sleep(Duration::from_millis(
                    runtime_config::notify_file_settle_ms(),
                ));
                match LocalBackend::new_item_from_path(path) {
                    Ok(Some(item)) => {
                        tracing::debug!(target: crate::core::log_targets::STORAGE, "增量 upsert 成功: {}", path.display());
                        if let Err(e) = db_actor.execute_blocking_in_trace(
                            trace.clone(),
                            DbCommand::UpsertMediaBatch {
                                source: ChangeSource::FilesystemWatcher,
                                items: vec![item],
                            },
                        ) {
                            tracing::warn!("actor upsert 失败 {}: {}", path.display(), e);
                        }
                    }
                    Ok(None) => {
                        // 非文件 / 已消失；不通知 UI。
                    }
                    Err(e) => {
                        log_warning(
                            &trace,
                            "metadata_extract",
                            format!("{}: {e}", path.display()),
                        );
                        tracing::warn!("upsert 失败 {}: {}", path.display(), e);
                    }
                }
            }
        }
        EventKind::Remove(_) => {
            for path in &evt.paths {
                if !is_supported_media_path(path) {
                    continue;
                }
                match db_actor.execute_blocking_in_trace(
                    trace.clone(),
                    DbCommand::DeleteLiveByPath {
                        source: ChangeSource::FilesystemWatcher,
                        path: path.clone(),
                    },
                ) {
                    Ok(crate::core::db_actor::DbCommandResult::RemovedUris(removed))
                        if !removed.is_empty() =>
                    {
                        tracing::debug!(target: crate::core::log_targets::STORAGE, "增量删除成功: {}", path.display());
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
                    std::thread::sleep(Duration::from_millis(
                        runtime_config::notify_file_settle_ms(),
                    ));
                    match LocalBackend::new_item_from_path(path) {
                        Ok(Some(item)) => {
                            tracing::debug!(target: crate::core::log_targets::STORAGE, "rename upsert 成功: {}", path.display());
                            if let Err(e) = db_actor.execute_blocking_in_trace(
                                trace.clone(),
                                DbCommand::UpsertMediaBatch {
                                    source: ChangeSource::FilesystemWatcher,
                                    items: vec![item],
                                },
                            ) {
                                tracing::warn!(
                                    "actor rename upsert 失败 {}: {}",
                                    path.display(),
                                    e
                                );
                            }
                        }
                        Ok(None) => {}
                        Err(e) => {
                            log_warning(
                                &trace,
                                "metadata_extract",
                                format!("{}: {e}", path.display()),
                            );
                            tracing::warn!("rename upsert 失败 {}: {}", path.display(), e);
                        }
                    }
                } else {
                    match db_actor.execute_blocking_in_trace(
                        trace.clone(),
                        DbCommand::DeleteLiveByPath {
                            source: ChangeSource::FilesystemWatcher,
                            path: path.clone(),
                        },
                    ) {
                        Ok(crate::core::db_actor::DbCommandResult::RemovedUris(removed))
                            if !removed.is_empty() =>
                        {
                            tracing::debug!(target: crate::core::log_targets::STORAGE, "rename delete 成功: {}", path.display());
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
mod tests;
