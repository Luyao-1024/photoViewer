use super::cache::cache_key_str;
use super::decode::{generate, pixbuf_is_light};
use super::{
    BackgroundPullState, LoadedThumb, LoaderState, PriItem, QueuedEntry, SharedQueue,
    SharedStatsDirtyCallback, TIER_BACKGROUND,
};
use crate::core::db::DbPool;
use gtk4::gdk::Texture;
use std::cmp::Reverse;
use std::path::PathBuf;
use std::sync::atomic::Ordering as AtomicOrdering;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tracing::{debug, warn};

pub(in crate::core::thumbnails) fn worker_loop(
    queue: SharedQueue,
    pool: DbPool,
    cache_dir: PathBuf,
    state: Arc<Mutex<LoaderState>>,
    bg: Arc<BackgroundPullState>,
    stats_dirty_callback: SharedStatsDirtyCallback,
) {
    while let Some(req) = next_request_or_pull(&queue, &pool, &bg, &state) {
        // `thumb:process` spans the worker's per-item work (queue pickup →
        // result), with `queue_wait_ms` recorded as a field. It parents the
        // `thumb:generate` span created inside `generate`.
        let process_span = tracing::debug_span!(
            "thumb:process",
            uri = %req.uri,
            size = ?req.size,
            tier = req.tier,
            media_id = req.media_id,
            queue_wait_ms = req.enqueued_at.elapsed().as_millis(),
        );
        let _process_guard = process_span.enter();
        match generate(&cache_dir, &req.uri, req.size, req.mtime) {
            Ok(pb) => {
                // 带 media_id 的请求生成成功后立刻标记，避免统计落后于可见缩略图。
                let generated_media_id = (req.media_id != 0).then_some(req.media_id);
                let is_light = pixbuf_is_light(&pb);
                let texture = Texture::for_pixbuf(&pb);
                let loaded = LoadedThumb {
                    texture: texture.clone(),
                    is_light,
                };
                let is_bg = req.tier >= TIER_BACKGROUND;
                let waiters = {
                    let mut st = match state.lock() {
                        Ok(s) => s,
                        Err(_) => return,
                    };
                    if !is_bg {
                        st.mem_cache.put(req.cache_key.clone(), loaded.clone());
                    }
                    st.in_flight.remove(&req.cache_key).unwrap_or_default()
                };
                debug!(
                    target: crate::core::log_targets::THUMBNAILS,
                    "THUMB_LOADER_TRACE worker_loaded uri={} size={:?} tier={} media_id={} texture={}x{} waiters={} cache_key={}",
                    req.uri,
                    req.size,
                    req.tier,
                    req.media_id,
                    pb.width(),
                    pb.height(),
                    waiters.len(),
                    req.cache_key
                );
                if let Some(media_id) = generated_media_id {
                    if let Err(e) = crate::core::db::mark_thumbnails_generated(&pool, &[media_id]) {
                        warn!("更新缩略图状态失败: {}", e);
                    } else if let Ok(callback) = stats_dirty_callback.lock() {
                        debug!(
                            target: crate::core::log_targets::THUMBNAILS,
                            "THUMB_LOADER_TRACE mark_generated media_id={} uri={}",
                            media_id,
                            req.uri
                        );
                        if let Some(callback) = callback.as_ref() {
                            callback();
                        }
                    }
                }
                for w in waiters {
                    let _ = w.send(loaded.clone());
                }
            }
            Err(e) => {
                drop_in_flight(&state, &req.cache_key);
                warn!(
                    target: crate::core::log_targets::THUMBNAILS,
                    "THUMB worker_failed uri={} size={:?} tier={} error={}",
                    req.uri,
                    req.size,
                    req.tier,
                    e
                );
            }
        }
    }
}

/// 取下一个工作项：优先队列（网格请求），队列空时从 DB 批量拉取
/// `worker_count` 条需生成的项一次性入队并唤醒所有 worker。
pub(in crate::core::thumbnails) fn next_request_or_pull(
    queue: &SharedQueue,
    pool: &DbPool,
    bg: &Arc<BackgroundPullState>,
    state: &Arc<Mutex<LoaderState>>,
) -> Option<PriItem> {
    let (lock, cvar) = &**queue;
    loop {
        // 1) 优先从队列弹（BOOST/NORMAL，网格可见请求）
        let mut q = lock.lock().ok()?;
        loop {
            if q.closed {
                return None;
            }
            if let Some(Reverse(item)) = q.heap.pop() {
                if q.queued.get(&item.cache_key).map(|e| e.tier) == Some(item.tier) {
                    q.queued.remove(&item.cache_key);
                    return Some(item);
                }
                continue; // 过期项
            }
            break; // 堆空
        }
        drop(q);

        // 2) 队列空，从 DB 批量拉取需生成的项
        if bg.enabled.load(AtomicOrdering::Relaxed) {
            if let Some(item) = pull_batch_and_enqueue(pool, bg, queue, state) {
                return Some(item);
            }
        }

        // 3) 无可做，阻塞等待。
        let q = lock.lock().ok()?;
        if q.closed {
            return None;
        }
        if !q.heap.is_empty() {
            continue;
        }
        let wait_dur = if bg.enabled.load(AtomicOrdering::Relaxed) {
            std::time::Duration::from_millis(
                crate::core::runtime_config::thumbnail_prewarm_poll_ms(),
            )
        } else {
            std::time::Duration::from_millis(crate::core::runtime_config::thumbnail_idle_wait_ms())
        };
        let (q2, _timed_out) = cvar.wait_timeout(q, wait_dur).ok()?;
        drop(q2);
    }
}

/// 从 DB 批量拉取 `worker_count` 条需生成的项，全部入队并唤醒其他 worker，
/// 返回一条给调用方自己处理（等价于调用方先从队里弹一条）。
///
/// 已缓存（`thumbnail_generated_at >= file_mtime`）的项由 DB 查询自动过滤，
/// 不再需要磁盘 stat。拉取到末尾返回 `None`；下次超时重试时会因为已缓存项增加
/// 而自然收敛。
pub(in crate::core::thumbnails) fn pull_batch_and_enqueue(
    pool: &DbPool,
    bg: &BackgroundPullState,
    queue: &SharedQueue,
    state: &Mutex<LoaderState>,
) -> Option<PriItem> {
    let batch_size = *bg.worker_count.lock().ok()? as u32;
    let start_offset = {
        let mut off = bg.offset.lock().ok()?;
        let start = *off;
        *off = off.saturating_add(batch_size);
        start
    };
    let page = crate::core::db::list_media_needing_thumbnail_from_live_offset(
        pool,
        start_offset,
        batch_size,
    )
    .ok()?;
    if page.is_empty() {
        if let Ok(mut off) = bg.offset.lock() {
            *off = 0;
        }
        return None;
    }

    let size = *bg.size.lock().ok()?;

    // 全部转成 PriItem，批量入队
    let items: Vec<PriItem> = page
        .iter()
        .map(|item| {
            let mtime = Some(std::time::SystemTime::from(item.file_mtime));
            let cache_key =
                cache_key_str(&item.uri, size, mtime).unwrap_or_else(|| format!("bg:{}", item.uri));
            PriItem {
                tier: TIER_BACKGROUND,
                seq: 0,
                cache_key,
                uri: item.uri.clone(),
                size,
                mtime,
                enqueued_at: Instant::now(),
                media_id: item.id,
            }
        })
        .collect();

    let (lock, cvar) = &**queue;
    let mut st = state.lock().ok()?;
    let mut q = lock.lock().ok()?;
    let mut first = None;
    for item in items {
        if st.in_flight.contains_key(&item.cache_key) || q.queued.contains_key(&item.cache_key) {
            continue;
        }
        if first.is_none() {
            st.in_flight.insert(item.cache_key.clone(), Vec::new());
            first = Some(item);
            continue;
        }
        if q.queued.len() < crate::core::runtime_config::thumbnail_queue_capacity() {
            st.in_flight.insert(item.cache_key.clone(), Vec::new());
            q.queued.insert(
                item.cache_key.clone(),
                QueuedEntry {
                    tier: TIER_BACKGROUND,
                    uri: item.uri.clone(),
                    size: item.size,
                    mtime: item.mtime,
                    enqueued_at: item.enqueued_at,
                    media_id: item.media_id,
                },
            );
            q.heap.push(Reverse(item));
        }
    }
    let queued_any = !q.heap.is_empty();
    drop(q);
    drop(st);
    if queued_any {
        // 唤醒所有 sleep 的 worker 来消费刚入队的项。
        cvar.notify_all();
    }

    first
}

/// 生成失败时移除在途项，让等待者的 `rx` 收到 `Err` 而非永久挂起。
pub(in crate::core::thumbnails) fn drop_in_flight(state: &Mutex<LoaderState>, cache_key: &str) {
    if let Ok(mut st) = state.lock() {
        st.in_flight.remove(cache_key);
    }
}
