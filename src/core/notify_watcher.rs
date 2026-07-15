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
use crate::core::media::is_supported_media_path;
use crate::core::runtime_config;
use crate::core::telemetry::{log_error, log_warning, OperationTrace, TraceChain};
use notify::{event::EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;
use tokio::task::JoinHandle;

/// 启动后台文件监听，返回一个 `JoinHandle`。
///
/// `watch_paths` 是要安装 inotify 的目录（相册目录 + 存在的回收站根）；
/// `trash_roots` 用于把事件分类成"回收站事件"（路径落在某个回收站根下）；
/// `pictures_root` 是对账时判断"原路径是否属于本图库"的根。
///
/// 监听在独立的阻塞线程中运行（`spawn_blocking`），不会阻塞 tokio / GTK 主循环。
pub fn start_watching(
    db_actor: DbActorHandle,
    watch_paths: Vec<PathBuf>,
    trash_roots: Vec<PathBuf>,
    excluded_roots: Vec<PathBuf>,
    pictures_root: PathBuf,
) -> JoinHandle<()> {
    tokio::task::spawn_blocking(move || {
        run_watcher_loop(
            db_actor,
            watch_paths,
            trash_roots,
            excluded_roots,
            pictures_root,
        )
    })
}

fn run_watcher_loop(
    db_actor: DbActorHandle,
    watch_paths: Vec<PathBuf>,
    trash_roots: Vec<PathBuf>,
    excluded_roots: Vec<PathBuf>,
    pictures_root: PathBuf,
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
    let mut trash_dirty = false;

    while let Ok(evt) = rx.recv() {
        dispatch_event(
            &db_actor,
            evt,
            &trash_roots,
            &excluded_roots,
            &mut trash_dirty,
        );

        // 排空本轮事件突发；静默配置的防抖时间（或通道关闭）后，若有回收站事件则
        // 对账 + 通知。
        while let Ok(e) = rx.recv_timeout(Duration::from_millis(
            runtime_config::notify_trash_debounce_ms(),
        )) {
            dispatch_event(
                &db_actor,
                e,
                &trash_roots,
                &excluded_roots,
                &mut trash_dirty,
            );
        }
        flush_trash_reconcile(&db_actor, &pictures_root, &mut trash_dirty);
    }
    // 通道关闭（停监）：把挂起的回收站变化最后冲刷一次再退出。
    flush_trash_reconcile(&db_actor, &pictures_root, &mut trash_dirty);
    drop(watcher);
}

/// 把一条事件分发到"回收站对账"或"相册增量 upsert/delete"。
///
/// 路径落在任一回收站根下 → 回收站事件（只置脏位，等防抖后批量对账）；否则按相册
/// 事件走 [`handle_event`]。
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
fn flush_trash_reconcile(db_actor: &DbActorHandle, pictures_root: &Path, trash_dirty: &mut bool) {
    if !*trash_dirty {
        return;
    }
    *trash_dirty = false;
    let trace = OperationTrace::start(TraceChain::Filesystem, "reconcile_trash");
    let result = db_actor.execute_blocking_in_trace(
        trace.clone(),
        DbCommand::ReconcileTrash {
            pictures_root: pictures_root.to_path_buf(),
        },
    );
    if let Err(error) = result {
        log_error(&trace, "db_reconcile", &error);
    }
}

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
